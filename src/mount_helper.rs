use crate::git_ops;
use crate::model::{Source, Workspace};
use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn remote_target_path(workspace: &Workspace, source: &Source) -> PathBuf {
    Path::new(&workspace.mountpoint).join(source.mount_name())
}

pub fn source_runtime_path(workspace: &Workspace, source: &Source) -> PathBuf {
    if source.is_remote_git() {
        remote_target_path(workspace, source)
    } else {
        PathBuf::from(&source.path)
    }
}

pub fn cleanup_mountpoint(mountpoint: &Path) -> Result<()> {
    if mountpoint.as_os_str().is_empty() {
        bail!("refusing to delete empty mountpoint path");
    }
    let abs = if mountpoint.is_absolute() {
        mountpoint.to_path_buf()
    } else {
        std::env::current_dir()?.join(mountpoint)
    };
    let metadata = match fs::symlink_metadata(&abs) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "refusing to remove non-directory mountpoint {}",
            abs.display()
        );
    }
    let resolved = fs::canonicalize(&abs)?;
    if resolved == Path::new("/") {
        bail!("refusing to delete filesystem root /");
    }
    if let Some(home) = dirs::home_dir() {
        if resolved == fs::canonicalize(&home).unwrap_or_else(|_| home.clone()) {
            bail!("refusing to delete home directory {}", home.display());
        }
    }
    // The mountpoint may also contain editor files or unrelated user data.
    // Only remove an empty directory; never recursively delete its contents.
    match fs::remove_dir(&abs) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
            eprintln!(
                "warning: mountpoint {} is not empty; preserving remaining files",
                abs.display()
            );
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// Check both sides of Git's worktree registration, not just the presence of
/// a .git file (which could belong to another repository or be forged).
fn is_managed_worktree(path: &Path, bare: &Path) -> bool {
    let real_dir = |path: &Path| {
        fs::symlink_metadata(path)
            .map(|m| m.is_dir() && !m.file_type().is_symlink())
            .unwrap_or(false)
    };
    let real_file = |path: &Path| {
        fs::symlink_metadata(path)
            .map(|m| m.is_file() && !m.file_type().is_symlink())
            .unwrap_or(false)
    };
    let git_file = path.join(".git");
    if !real_dir(path) || !real_dir(bare) || !real_file(&git_file) {
        return false;
    }
    let Ok(contents) = fs::read_to_string(&git_file) else {
        return false;
    };
    let Some(git_dir) = contents.trim().strip_prefix("gitdir: ") else {
        return false;
    };
    let git_dir = path.join(git_dir);
    let registrations = bare.join("worktrees");
    if !real_dir(&git_dir) || !real_dir(&registrations) {
        return false;
    }
    let (Ok(actual), Ok(expected), Ok(bare), Ok(git_file)) = (
        fs::canonicalize(&git_dir),
        fs::canonicalize(&registrations),
        fs::canonicalize(bare),
        fs::canonicalize(&git_file),
    ) else {
        return false;
    };
    if actual.parent() != Some(expected.as_path()) {
        return false;
    }
    let points_to = |file: &str, target: &Path| {
        let file = actual.join(file);
        real_file(&file)
            && fs::read_to_string(file)
                .ok()
                .and_then(|s| fs::canonicalize(actual.join(s.trim())).ok())
                .is_some_and(|resolved| resolved == target)
    };
    points_to("gitdir", &git_file) && points_to("commondir", &bare)
}

pub fn cleanup_worktrees(workspace: &Workspace) -> Result<usize> {
    workspace.validate_mount_names()?;
    // Validate every target before removing the first worktree. A failed Git
    // removal must leave the directory intact, never fall back to recursive rm.
    let mut targets = Vec::new();
    for src in workspace.sources.iter().filter(|src| src.is_remote_git()) {
        let path = remote_target_path(workspace, src);
        match fs::symlink_metadata(&path) {
            Ok(_) => (),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        }
        let mut component_path = PathBuf::from(&workspace.mountpoint);
        for component in Path::new(&src.mount_name()).components() {
            component_path.push(component);
            if fs::symlink_metadata(&component_path)?
                .file_type()
                .is_symlink()
            {
                bail!(
                    "refusing to remove worktree through symlink {}",
                    component_path.display()
                );
            }
        }
        let resolved = fs::canonicalize(&path)?;
        for local in workspace.sources.iter().filter(|s| !s.is_remote_git()) {
            let local_path = Path::new(&local.path);
            // Fail closed if a configured local source cannot be resolved.
            let local_path = fs::canonicalize(local_path)?;
            if local_path.starts_with(&resolved) {
                bail!(
                    "refusing to remove worktree {} containing local source {}",
                    path.display(),
                    local_path.display()
                );
            }
        }
        let remote = src.remote_git.as_ref().expect("filtered remote source");
        let bare =
            crate::url_parser::default_bare_path_isolated(&remote.url, &workspace.name, None)
                .ok_or_else(|| anyhow::anyhow!("cannot determine managed bare repository"))?;
        if !is_managed_worktree(&path, &bare) {
            bail!(
                "refusing to remove {}, which is not a verified vwsd worktree of {}",
                path.display(),
                bare.display()
            );
        }
        targets.push((path, bare));
    }
    let mut removed = 0;
    for (path, bare) in targets {
        git_ops::worktree_remove(&bare, &path)?;
        removed += 1;
    }
    Ok(removed)
}

pub fn is_bare_repo_used_by_others(url: &str, exclude_workspace: &str) -> bool {
    let ws_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".vws")
        .join("workspaces");
    let Ok(entries) = fs::read_dir(&ws_dir) else {
        return true;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return true;
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(ws) = serde_json::from_str::<Workspace>(&text) {
                if ws.name == exclude_workspace {
                    continue;
                }
                if ws.sources.iter().any(|src| {
                    src.remote_git
                        .as_ref()
                        .map(|rg| rg.url == url)
                        .unwrap_or(false)
                }) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn cleanup_bare_repo_if_unused(url: &str, exclude_workspace: &str) -> Result<()> {
    if is_bare_repo_used_by_others(url, exclude_workspace) {
        return Ok(());
    }
    if let Some(shared) = crate::url_parser::default_bare_path(url, None) {
        if shared.exists() {
            fs::remove_dir_all(&shared)?;
        }
        let lock = shared.with_extension("lock");
        if lock.exists() {
            let _ = fs::remove_file(lock);
        }
    }
    Ok(())
}
