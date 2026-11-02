/// Expand a leading `~` in a path string to the user's home directory.
///
/// - `"~"` → home directory
/// - `"~/foo"` → `<home>/foo`
/// - anything else → unchanged
pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    } else if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy().to_string();
            return path.replacen("~", &home_str, 1);
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_home_only() {
        let result = expand_tilde("~");
        assert!(
            !result.contains('~'),
            "expected ~ to be expanded, got {}",
            result
        );
        assert!(!result.is_empty());
    }

    #[test]
    fn expand_tilde_with_subpath() {
        let result = expand_tilde("~/projects/foo");
        assert!(
            !result.starts_with("~/"),
            "expected ~ to be expanded, got {}",
            result
        );
        assert!(result.ends_with("/projects/foo"));
    }

    #[test]
    fn expand_tilde_absolute_path_unchanged() {
        assert_eq!(expand_tilde("/usr/local/bin"), "/usr/local/bin");
    }

    #[test]
    fn expand_tilde_relative_path_unchanged() {
        assert_eq!(expand_tilde("relative/path"), "relative/path");
    }

    #[test]
    fn expand_tilde_empty_unchanged() {
        assert_eq!(expand_tilde(""), "");
    }

    #[test]
    fn expand_tilde_tilde_in_middle_unchanged() {
        assert_eq!(expand_tilde("/some/~/path"), "/some/~/path");
    }
}
