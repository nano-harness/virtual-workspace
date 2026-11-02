use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::git_ops;
use crate::model::Source;
use crate::url_parser;

fn resolve_root(cache_root: Option<&Path>) -> PathBuf {
    cache_root.map(PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".vws")
    })
}

pub fn shared_bare_path(url: &str, cache_root: Option<&Path>) -> Result<PathBuf> {
    let root = resolve_root(cache_root);
    url_parser::default_bare_path(url, Some(&root))
        .context("cannot derive shared bare repository path from git URL")
}

pub fn isolated_bare_path(
    url: &str,
    workspace_name: &str,
    cache_root: Option<&Path>,
) -> Result<PathBuf> {
    let root = resolve_root(cache_root);
    url_parser::default_bare_path_isolated(url, workspace_name, Some(&root))
        .context("cannot derive isolated bare repository path from git URL")
}

fn is_git_worktree(path: &Path) -> bool {
    let git = path.join(".git");
    git.exists() && git.is_file()
}

fn acquire_lock(bare: &Path) -> Result<fs::File> {
    let lock_path = bare.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open lock file {}", lock_path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("failed to acquire lock on {}", lock_path.display()))?;
    Ok(file)
}

const SHARED_FETCH_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

fn mark_shared_fetched(shared: &Path) {
    let _ = fs::write(shared.join("FETCH_HEAD"), b"");
}

fn shared_recently_fetched(shared: &Path, window: std::time::Duration) -> bool {
    fs::metadata(shared.join("FETCH_HEAD"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| mtime.elapsed().ok())
        .map(|age| age < window)
        .unwrap_or(false)
}

fn isolated_uses_shared_pool(isolated: &Path, shared: &Path) -> bool {
    let expected =
        fs::canonicalize(shared.join("objects")).unwrap_or_else(|_| shared.join("objects"));
    fs::read_to_string(isolated.join("objects").join("info").join("alternates"))
        .map(|content| {
            content.lines().any(|line| {
                let line = line.trim();
                !line.is_empty()
                    && fs::canonicalize(line).unwrap_or_else(|_| PathBuf::from(line)) == expected
            })
        })
        .unwrap_or(false)
}

fn adopt_shared_pool(isolated: &Path, shared: &Path) {
    if isolated_uses_shared_pool(isolated, shared) {
        return;
    }
    let alternates = isolated.join("objects").join("info").join("alternates");
    if let Some(parent) = alternates.parent() {
        let _ = fs::create_dir_all(parent);
    }
    use std::io::Write;
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&alternates)
    {
        let objects =
            fs::canonicalize(shared.join("objects")).unwrap_or_else(|_| shared.join("objects"));
        let _ = writeln!(file, "{}", objects.display());
    }
}

fn ensure_shared_fresh(shared: &Path) -> Result<()> {
    if shared_recently_fetched(shared, SHARED_FETCH_WINDOW) {
        return Ok(());
    }
    let _pool_lock = acquire_lock(shared)?;
    if shared_recently_fetched(shared, SHARED_FETCH_WINDOW) {
        return Ok(());
    }
    if git_ops::fix_fetch_refspec(shared)? {
        git_ops::migrate_refs_to_remotes(shared)?;
    }
    git_ops::fetch_all(shared)
        .with_context(|| format!("git fetch failed for shared repo at {}", shared.display()))
}

fn refresh_isolated_refs(isolated: &Path, shared: &Path) -> Result<()> {
    if shared.exists() && isolated_uses_shared_pool(isolated, shared) {
        ensure_shared_fresh(shared)?;
        git_ops::sync_refs_from(isolated, shared).context("failed to sync refs from shared repo")
    } else {
        git_ops::fetch_all(isolated)
            .with_context(|| format!("git fetch failed for bare repo at {}", isolated.display()))?;
        if shared.exists() {
            adopt_shared_pool(isolated, shared);
        }
        Ok(())
    }
}

/// Pick an existing remote-tracking branch, preferring `main`/`master`.
/// Used when the bare repo's HEAD points at an unborn branch (e.g. a fresh
/// remote whose HEAD was never updated to the pushed default branch).
fn existing_remote_branch(bare: &Path) -> Result<String> {
    let branches = git_ops::remote_branches(bare)?;
    for preferred in ["main", "master"] {
        if branches.iter().any(|b| b == preferred) {
            return Ok(preferred.to_string());
        }
    }
    branches.into_iter().next().context(
        "no remote-tracking branches found in bare repo; set `branch` explicitly in the config",
    )
}

fn target_branch(configured: Option<&str>, bare: &Path) -> Result<String> {
    if let Some(branch) = configured {
        return Ok(branch.to_string());
    }
    let head = git_ops::default_branch(bare)
        .context("cannot determine default branch from bare repo; set `branch` explicitly")?;
    let tracking = format!("refs/remotes/origin/{}", head);
    if git_ops::ref_exists(bare, &tracking).unwrap_or(false) {
        return Ok(head);
    }
    // The remote's HEAD may refer to a branch that was never pushed
    // (e.g. HEAD says `master` but only `main` exists); fall back to a
    // remote branch that actually exists instead of failing the mount.
    existing_remote_branch(bare).with_context(|| {
        format!(
            "default branch '{}' not found on the remote; set `branch` explicitly in the config",
            head
        )
    })
}

pub fn prepare_remote_source(
    source: &mut Source,
    workspace_name: &str,
    target_path: &Path,
    cache_root: Option<&Path>,
) -> Result<(PathBuf, bool)> {
    let rg = source
        .remote_git
        .clone()
        .context("prepare_remote_source called for non-remote source")?;

    if source.mount_as.is_none() {
        if let Some(parsed) = url_parser::parse_git_url(&rg.url) {
            source.mount_as = Some(parsed.repo);
        }
    }

    let shared_bare = shared_bare_path(&rg.url, cache_root)?;
    let bare = isolated_bare_path(&rg.url, workspace_name, cache_root)?;
    let _lock = acquire_lock(&bare)?;

    let mut branch = if bare.exists() {
        if git_ops::fix_fetch_refspec(&bare)? {
            git_ops::migrate_refs_to_remotes(&bare)?;
        }
        refresh_isolated_refs(&bare, &shared_bare)?;
        let branch = target_branch(rg.branch.as_deref(), &bare)?;
        let tracking = format!("refs/remotes/origin/{}", branch);
        if !git_ops::ref_exists(&bare, &tracking).unwrap_or(false) {
            git_ops::migrate_refs_to_remotes(&bare)?;
        }
        branch
    } else {
        if let Some(parent) = bare.parent() {
            fs::create_dir_all(parent)?;
        }
        if !shared_bare.exists() {
            if let Some(parent) = shared_bare.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = git_ops::clone_bare(&rg.url, &shared_bare);
            if shared_bare.exists() {
                mark_shared_fetched(&shared_bare);
            }
        }
        if shared_bare.exists() && shared_bare != bare {
            ensure_shared_fresh(&shared_bare)?;
            git_ops::clone_bare_from_shared(&shared_bare, &bare, &rg.url).with_context(|| {
                format!(
                    "failed to clone isolated bare repo from shared pool {} (remote: {})",
                    shared_bare.display(),
                    rg.url
                )
            })?;
        } else {
            git_ops::clone_bare(&rg.url, &bare)
                .with_context(|| format!("failed to clone bare repo from {}", rg.url))?;
        }
        git_ops::migrate_refs_to_remotes(&bare)?;
        target_branch(rg.branch.as_deref(), &bare)?
    };

    let mut branch_changed = false;
    if !git_ops::branch_exists(&bare, &branch)? {
        let remote_ref = format!("refs/remotes/origin/{}", branch);
        let start_point = if git_ops::ref_exists(&bare, &remote_ref)? {
            remote_ref
        } else {
            let default = target_branch(None, &bare)?;
            let remote_default = format!("refs/remotes/origin/{}", default);
            if git_ops::ref_exists(&bare, &remote_default)? {
                remote_default
            } else {
                default
            }
        };
        git_ops::create_branch(&bare, &branch, &start_point)?;
    }
    git_ops::set_upstream(&bare, &branch, "origin")?;

    // Drop stale worktree registrations (e.g. a worktree directory the user
    // deleted by hand) so `worktree add` below does not fail on them.
    let _ = git_ops::worktree_prune(&bare);

    if is_git_worktree(target_path) {
        let current = git_ops::current_branch(target_path).unwrap_or_default();
        if current.is_empty() {
            git_ops::worktree_checkout_branch(target_path, &branch).with_context(|| {
                format!(
                    "failed to checkout branch '{}' in worktree at {}",
                    branch,
                    target_path.display()
                )
            })?;
        } else if current != branch {
            branch = current.clone();
            if let Some(remote_git) = &mut source.remote_git {
                remote_git.branch = Some(current.clone());
            }
            branch_changed = true;
            let _ = git_ops::set_upstream(&bare, &branch, "origin");
        }
    } else if target_path.exists() {
        if target_path.join(".git").is_dir() {
            bail!(
                "path {} contains a regular git clone, not a worktree; please remove it and retry",
                target_path.display()
            );
        }
        let mut entries = fs::read_dir(target_path)
            .with_context(|| format!("failed to read contents of {}", target_path.display()))?;
        if entries.next().is_some() {
            bail!(
                "worktree path {} exists, is not a git worktree, and is not empty; please remove it or choose a different mountpoint/mount_as",
                target_path.display()
            );
        }
        fs::remove_dir(target_path)?;
        git_ops::worktree_add(&bare, target_path, &branch)
            .with_context(|| format!("failed to create worktree at {}", target_path.display()))?;
    } else {
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        git_ops::worktree_add(&bare, target_path, &branch)
            .with_context(|| format!("failed to create worktree at {}", target_path.display()))?;
    }

    Ok((target_path.to_path_buf(), branch_changed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RemoteGit;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_bare_with_commit(td: &Path) -> PathBuf {
        let bare = td.join("origin.git");
        Command::new("git")
            .args(["init", "--bare"])
            .arg(&bare)
            .output()
            .unwrap();
        let seed = td.join("seed");
        git_ops::clone(bare.to_str().unwrap(), &seed, None).unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(&seed)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(&seed)
            .output()
            .unwrap();
        fs::write(seed.join("readme.md"), "# test").unwrap();
        git_ops::add_all(&seed).unwrap();
        git_ops::commit(&seed, "init").unwrap();
        git_ops::push(&seed).unwrap();
        bare
    }

    #[test]
    fn prepare_remote_source_creates_bare_and_worktree() {
        let td = TempDir::new().unwrap();
        let bare_remote = init_bare_with_commit(td.path());
        let root = td.path().join("vws");
        let target = td.path().join("mount").join("repo");
        let mut src = Source {
            path: String::new(),
            remote_git: Some(RemoteGit {
                url: bare_remote.to_string_lossy().to_string(),
                branch: None,
            }),
            mount_as: Some("repo".to_string()),
        };

        let (resolved, _) =
            prepare_remote_source(&mut src, "test-ws", &target, Some(&root)).unwrap();
        assert_eq!(resolved, target);
        assert!(resolved.join("readme.md").exists());
        assert!(resolved.join(".git").is_file());
        assert!(
            shared_bare_path(&src.remote_git.as_ref().unwrap().url, Some(&root))
                .unwrap()
                .exists()
        );
        assert!(isolated_bare_path(
            &src.remote_git.as_ref().unwrap().url,
            "test-ws",
            Some(&root)
        )
        .unwrap()
        .exists());
    }

    #[test]
    fn prepare_remote_source_falls_back_when_remote_head_is_unborn() {
        let td = TempDir::new().unwrap();
        let bare_remote = init_bare_with_commit(td.path());
        // Which branch got pushed depends on the machine's git defaults.
        let pushed = Command::new("git")
            .args(["for-each-ref", "--format=%(refname:strip=2)", "refs/heads"])
            .current_dir(&bare_remote)
            .output()
            .unwrap();
        let pushed = String::from_utf8(pushed.stdout).unwrap().trim().to_string();
        assert!(!pushed.is_empty());
        // Point the remote's HEAD at a branch that does not exist, mimicking
        // a bare remote whose HEAD was never updated after the first push.
        Command::new("git")
            .args(["symbolic-ref", "HEAD", "refs/heads/definitely-missing"])
            .current_dir(&bare_remote)
            .output()
            .unwrap();

        let root = td.path().join("vws");
        let target = td.path().join("mount").join("repo");
        let mut src = Source {
            path: String::new(),
            remote_git: Some(RemoteGit {
                url: bare_remote.to_string_lossy().to_string(),
                branch: None,
            }),
            mount_as: Some("repo".to_string()),
        };

        let (resolved, _) =
            prepare_remote_source(&mut src, "test-ws", &target, Some(&root)).unwrap();
        assert!(resolved.join("readme.md").exists());
        assert_eq!(git_ops::current_branch(&resolved).unwrap(), pushed);
    }

    #[test]
    fn prepare_remote_source_rejects_nonempty_target() {
        let td = TempDir::new().unwrap();
        let bare_remote = init_bare_with_commit(td.path());
        let root = td.path().join("vws");
        let target = td.path().join("mount").join("repo");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("stale.txt"), "stale").unwrap();
        let mut src = Source {
            path: String::new(),
            remote_git: Some(RemoteGit {
                url: bare_remote.to_string_lossy().to_string(),
                branch: None,
            }),
            mount_as: Some("repo".to_string()),
        };

        let err = prepare_remote_source(&mut src, "test-ws", &target, Some(&root)).unwrap_err();
        assert!(err.to_string().contains("is not empty"));
    }
}
