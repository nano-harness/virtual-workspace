use std::path::{Path, PathBuf};

/// Parsed components from a git URL.
#[derive(Debug, Clone, PartialEq)]
pub struct GitUrl {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

/// Parse a git URL (HTTPS or SSH) into its components.
///
/// Supported formats:
/// - `https://github.com/user/repo.git`
/// - `https://github.com/user/repo`
/// - `git@github.com:user/repo.git`
/// - `ssh://git@github.com/user/repo.git`
pub fn parse_git_url(url: &str) -> Option<GitUrl> {
    let url = url.trim();

    // SSH shorthand: git@host:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@") {
        if let Some(colon_pos) = rest.find(':') {
            let host = &rest[..colon_pos];
            let path = &rest[colon_pos + 1..];
            return parse_owner_repo(host, path);
        }
        return None;
    }

    // ssh://git@host/owner/repo.git
    if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.strip_prefix("git@").unwrap_or(rest);
        if let Some(slash_pos) = rest.find('/') {
            let host = &rest[..slash_pos];
            let path = &rest[slash_pos + 1..];
            return parse_owner_repo(host, path);
        }
        return None;
    }

    // https://host/owner/repo.git or http://host/owner/repo.git
    for prefix in &["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            if let Some(slash_pos) = rest.find('/') {
                let host = &rest[..slash_pos];
                let path = &rest[slash_pos + 1..];
                return parse_owner_repo(host, path);
            }
            return None;
        }
    }

    None
}

fn parse_owner_repo(host: &str, path: &str) -> Option<GitUrl> {
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_matches('/');
    let parts: Vec<&str> = path.splitn(2, '/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some(GitUrl {
            host: host.to_string(),
            owner: parts[0].to_string(),
            repo: parts[1].to_string(),
        })
    } else {
        None
    }
}

/// Sanitize a single component for use in a cache directory name.
///
/// Rejects path-traversal segments (`.`, `..`) and replaces path separators
/// (`/`, `\`) with underscores so the resulting string is safe as a single
/// directory name component.
fn sanitize_cache_component(component: &str) -> Option<String> {
    if component.is_empty() || component == "." || component == ".." {
        return None;
    }

    let mut sanitized = String::with_capacity(component.len());
    for ch in component.chars() {
        match ch {
            '/' | '\\' => sanitized.push('_'),
            _ => sanitized.push(ch),
        }
    }

    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

/// Derive a local cache directory name from a git URL.
///
/// Returns a sanitised string like `github.com_user_repo` suitable for use
/// as a directory name under `~/.vws/git_cache/`.
///
/// All components are sanitized to prevent path traversal or nested directory
/// creation (e.g., GitLab subgroups with `/` are flattened).
pub fn cache_key(url: &str) -> Option<String> {
    parse_git_url(url).and_then(|g| {
        let host = sanitize_cache_component(&g.host)?;
        let owner = sanitize_cache_component(&g.owner)?;
        let repo = sanitize_cache_component(&g.repo)?;
        Some(format!("{}_{}_{}", host, owner, repo))
    })
}

/// Derive a cache key for bare repository storage.
///
/// Tries standard git URL parsing first, then falls back to sanitizing local
/// file paths so that bare-clone caching works in tests and with `file://` URLs.
pub fn bare_cache_key(url: &str) -> Option<String> {
    if let Some(key) = cache_key(url) {
        return Some(key);
    }
    // Fallback for local file paths (e.g. used in tests)
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() {
        return None;
    }
    let sanitized: String = url
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '.' => '_',
            _ => c,
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        None
    } else {
        Some(format!("local_{}", trimmed))
    }
}

/// Return the base directory for git bare repo caches.
/// Typically `<root>/git_cache`.
pub fn git_cache_dir(root: Option<&Path>) -> Option<PathBuf> {
    let base = root
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".vws")))?;
    Some(base.join("git_cache"))
}

/// Return the default bare repository path for a given git URL.
///
/// Bare repos are stored at `<root>/git_cache/<key>.git`.
/// When `root` is `None` the default `~/.vws` is used.
pub fn default_bare_path(url: &str, root: Option<&Path>) -> Option<PathBuf> {
    let key = bare_cache_key(url)?;
    let base = root
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".vws")))?;
    Some(base.join("git_cache").join(format!("{}.git", key)))
}

/// Return an isolated bare repository path scoped to a specific workspace.
///
/// Used when the same `(url, branch)` is shared by multiple workspaces:
/// each workspace gets its own bare repo to avoid git worktree branch
/// conflicts.  Path: `<root>/git_cache/<key>__<ws_name>.git`.
pub fn default_bare_path_isolated(
    url: &str,
    ws_name: &str,
    root: Option<&Path>,
) -> Option<PathBuf> {
    let key = bare_cache_key(url)?;
    let cache_dir = git_cache_dir(root)?;
    let ws_name = sanitize_cache_component(ws_name)?;
    Some(cache_dir.join(format!("{}__{}.git", key, ws_name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_https_url() {
        let g = parse_git_url("https://github.com/user/repo.git").unwrap();
        assert_eq!(g.host, "github.com");
        assert_eq!(g.owner, "user");
        assert_eq!(g.repo, "repo");
    }

    #[test]
    fn parse_https_url_no_dot_git() {
        let g = parse_git_url("https://github.com/user/repo").unwrap();
        assert_eq!(g.host, "github.com");
        assert_eq!(g.owner, "user");
        assert_eq!(g.repo, "repo");
    }

    #[test]
    fn parse_ssh_shorthand() {
        let g = parse_git_url("git@github.com:org/project.git").unwrap();
        assert_eq!(g.host, "github.com");
        assert_eq!(g.owner, "org");
        assert_eq!(g.repo, "project");
    }

    #[test]
    fn parse_ssh_full() {
        let g = parse_git_url("ssh://git@gitlab.com/team/lib.git").unwrap();
        assert_eq!(g.host, "gitlab.com");
        assert_eq!(g.owner, "team");
        assert_eq!(g.repo, "lib");
    }

    #[test]
    fn cache_key_works() {
        assert_eq!(
            cache_key("https://github.com/user/repo.git"),
            Some("github.com_user_repo".to_string())
        );
    }

    #[test]
    fn invalid_url_returns_none() {
        assert!(parse_git_url("not-a-url").is_none());
        assert!(parse_git_url("https://github.com").is_none());
        assert!(parse_git_url("").is_none());
    }

    #[test]
    fn bare_cache_key_remote_url() {
        assert_eq!(
            bare_cache_key("https://github.com/user/repo.git"),
            Some("github.com_user_repo".to_string())
        );
    }

    #[test]
    fn bare_cache_key_local_path() {
        let key = bare_cache_key("/tmp/test/origin.git").unwrap();
        assert!(key.starts_with("local_"));
        assert!(key.contains("tmp"));
        assert!(key.contains("origin"));
    }

    #[test]
    fn bare_cache_key_empty_returns_none() {
        assert!(bare_cache_key("").is_none());
    }

    #[test]
    fn default_bare_path_isolated_generates_workspace_specific_path() {
        use std::path::PathBuf;
        let root = PathBuf::from("/test/root");
        let url = "https://github.com/user/repo.git";
        let ws_name = "my-workspace";

        let path = default_bare_path_isolated(url, ws_name, Some(&root)).unwrap();

        // Should contain workspace name in the path
        assert!(path.to_string_lossy().contains("my-workspace"));
        // Should be in git_cache directory
        assert!(path.to_string_lossy().contains("git_cache"));
        // Should have .git extension
        assert!(path.to_string_lossy().ends_with(".git"));
        // Should contain the __ separator
        assert!(path.to_string_lossy().contains("__"));
    }

    #[test]
    fn default_bare_path_isolated_different_workspaces_different_paths() {
        use std::path::PathBuf;
        let root = PathBuf::from("/test/root");
        let url = "https://github.com/user/repo.git";

        let path1 = default_bare_path_isolated(url, "ws-a", Some(&root)).unwrap();
        let path2 = default_bare_path_isolated(url, "ws-b", Some(&root)).unwrap();

        // Different workspaces should get different paths
        assert_ne!(path1, path2);
        assert!(path1.to_string_lossy().contains("ws-a"));
        assert!(path2.to_string_lossy().contains("ws-b"));
    }

    #[test]
    fn default_bare_path_isolated_sanitizes_workspace_name() {
        use std::path::PathBuf;
        let root = PathBuf::from("/test/root");
        let url = "https://github.com/user/repo.git";

        let path = default_bare_path_isolated(url, "team/ws", Some(&root)).unwrap();

        assert_eq!(path.parent(), Some(root.join("git_cache").as_path()));
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("github.com_user_repo__team_ws.git")
        );
    }
}
