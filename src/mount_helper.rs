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
    let path_str = mountpoint.to_string_lossy();
    if path_str.is_empty() {
        bail!("refusing to delete empty mountpoint path");
    }
    let abs = if mountpoint.is_absolute() {
        mountpoint.to_path_buf()
    } else {
        std::env::current_dir()?.join(mountpoint)
    };
    if abs == Path::new("/") {
        bail!("refusing to delete filesystem root /");
    }
    if let Some(home) = dirs::home_dir() {
        if abs == home {
            bail!("refusing to delete home directory {}", home.display());
        }
    }
    if mountpoint.exists() {
        fs::remove_dir_all(mountpoint)?;
    }
    Ok(())
}

pub fn cleanup_worktrees(workspace: &Workspace) -> Result<usize> {
    let mut removed = 0usize;
    for src in workspace.sources.iter().filter(|src| src.is_remote_git()) {
        let path = remote_target_path(workspace, src);
        if !path.exists() {
            continue;
        }
        if let Some(rg) = &src.remote_git {
            if let Some(bare) =
                crate::url_parser::default_bare_path_isolated(&rg.url, &workspace.name, None)
            {
                if bare.exists() {
                    let _ = git_ops::worktree_remove(&bare, &path);
                }
            }
        }
        if path.exists() {
            // Only force-delete directories that are actually git worktrees
            // (`.git` is a file, not a directory). A regular clone or a plain
            // directory with user data must never be removed here.
            if path.join(".git").is_file() {
                fs::remove_dir_all(&path)?;
            } else {
                eprintln!(
                    "warning: {} exists but is not a git worktree created by vwsd; leaving it in place",
                    path.display()
                );
                continue;
            }
        }
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
