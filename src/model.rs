use serde::{Deserialize, Serialize};

use crate::util::expand_tilde;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGit {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_git: Option<RemoteGit>,
    /// Name used for the remote worktree directory under the workspace mountpoint.
    /// Local sources use this only as a display label; their actual on-disk path
    /// remains `path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_as: Option<String>,
}

impl Source {
    pub fn mount_name(&self) -> String {
        if let Some(ref name) = self.mount_as {
            return name.clone();
        }
        if let Some(remote_git) = &self.remote_git {
            if let Some(parsed) = crate::url_parser::parse_git_url(&remote_git.url) {
                return parsed.repo;
            }
            // Local path or file:// remote: derive the name from its last
            // path component, stripping a trailing `.git`.
            let trimmed = remote_git.url.trim().trim_end_matches('/');
            let stripped = trimmed.strip_prefix("file://").unwrap_or(trimmed);
            if let Some(name) = std::path::Path::new(stripped)
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".git").or(Some(n)))
                .filter(|n| !n.is_empty())
            {
                return name.to_string();
            }
        }
        std::path::Path::new(&self.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "unnamed".to_string())
    }

    pub fn is_remote_git(&self) -> bool {
        self.remote_git.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub mountpoint: String,
    pub sources: Vec<Source>,
}

impl Workspace {
    pub fn ensure_id(&mut self) -> bool {
        if self.id.is_empty() {
            self.id = uuid::Uuid::new_v4().to_string();
            true
        } else {
            false
        }
    }

    /// Reject mount names that could escape the mountpoint when joined to it.
    ///
    /// A mount name becomes `<mountpoint>/<name>` for remote sources, so it must
    /// be a relative path made of plain components: no absolute paths, no `.` or
    /// `..`, no root/prefix. Nested names like `group/repo` are allowed because
    /// they still resolve inside the mountpoint.
    pub fn validate_mount_names(&self) -> anyhow::Result<()> {
        for src in &self.sources {
            let name = src.mount_name();
            let path = std::path::Path::new(&name);
            let safe = !name.trim().is_empty()
                && !path.is_absolute()
                && path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)));
            if !safe {
                anyhow::bail!(
                    "invalid mount name {:?}: it must be a relative path of plain directory names \
                     (no absolute paths, '.', or '..'); fix `mount_as` in the workspace config",
                    name
                );
            }
        }
        Ok(())
    }

    /// Expand `~` in `mountpoint` and all `sources[].path` values.
    pub fn expand_paths(&mut self) {
        self.mountpoint = expand_tilde(&self.mountpoint);
        for src in &mut self.sources {
            src.path = expand_tilde(&src.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_name_from_path() {
        let src = Source {
            path: "/home/user/projects/my-app".to_string(),
            remote_git: None,
            mount_as: None,
        };
        assert_eq!(src.mount_name(), "my-app");
    }

    #[test]
    fn mount_name_from_mount_as() {
        let src = Source {
            path: String::new(),
            remote_git: None,
            mount_as: Some("custom-name".to_string()),
        };
        assert_eq!(src.mount_name(), "custom-name");
    }

    #[test]
    fn mount_name_from_local_path_remote() {
        let make = |url: &str| Source {
            path: String::new(),
            remote_git: Some(RemoteGit {
                url: url.to_string(),
                branch: None,
            }),
            mount_as: None,
        };
        assert_eq!(make("/tmp/cache/origin.git").mount_name(), "origin");
        assert_eq!(make("file:///home/u/repos/lib.git").mount_name(), "lib");
        assert_eq!(make("/home/u/repos/plain").mount_name(), "plain");
    }

    #[test]
    fn mount_name_empty_path_no_mount_as() {
        let src = Source {
            path: String::new(),
            remote_git: None,
            mount_as: None,
        };
        assert_eq!(src.mount_name(), "unnamed");
    }

    #[test]
    fn expand_paths_expands_tilde_in_all_fields() {
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        let mut ws = Workspace {
            id: String::new(),
            name: "test".to_string(),
            mountpoint: "~/mnt/ws".to_string(),
            sources: vec![
                Source {
                    path: "~/code/a".to_string(),
                    remote_git: None,
                    mount_as: None,
                },
                Source {
                    path: "/absolute/path".to_string(),
                    remote_git: None,
                    mount_as: None,
                },
            ],
        };
        ws.expand_paths();
        assert_eq!(ws.mountpoint, format!("{}/mnt/ws", home));
        assert_eq!(ws.sources[0].path, format!("{}/code/a", home));
        assert_eq!(ws.sources[1].path, "/absolute/path");
    }

    #[test]
    fn legacy_workspace_without_id_deserializes_and_ensure_id_populates() {
        let json = r#"{
            "name": "legacy",
            "mountpoint": "/mnt/ws",
            "mode": "cow",
            "upperdir": "/tmp/upper",
            "filters": {"include": [], "exclude": []},
            "sources": []
        }"#;
        let mut ws: Workspace = serde_json::from_str(json).unwrap();
        assert!(ws.id.is_empty());
        assert!(ws.ensure_id());
        assert_eq!(ws.id.len(), 36);
        assert!(!ws.ensure_id());
    }

    #[test]
    fn validate_mount_names_rejects_escapes() {
        let ws_with = |mount_as: Option<&str>| Workspace {
            id: String::new(),
            name: "test".to_string(),
            mountpoint: "/mnt/ws".to_string(),
            sources: vec![Source {
                path: String::new(),
                remote_git: Some(RemoteGit {
                    url: "https://github.com/example/repo".to_string(),
                    branch: None,
                }),
                mount_as: mount_as.map(str::to_string),
            }],
        };

        // Plain and nested relative names are fine.
        assert!(ws_with(Some("repo")).validate_mount_names().is_ok());
        assert!(ws_with(Some("group/repo")).validate_mount_names().is_ok());
        assert!(ws_with(None).validate_mount_names().is_ok());

        // Anything that escapes the mountpoint is rejected.
        assert!(ws_with(Some("..")).validate_mount_names().is_err());
        assert!(ws_with(Some("../etc")).validate_mount_names().is_err());
        assert!(ws_with(Some("/etc")).validate_mount_names().is_err());
        assert!(ws_with(Some(".")).validate_mount_names().is_err());
        assert!(ws_with(Some("")).validate_mount_names().is_err());
        assert!(ws_with(Some("a/../../b")).validate_mount_names().is_err());
    }

    #[test]
    fn is_remote_git_detects_source_type() {
        let local = Source {
            path: "/test".to_string(),
            remote_git: None,
            mount_as: None,
        };
        assert!(!local.is_remote_git());

        let src = Source {
            path: String::new(),
            remote_git: Some(RemoteGit {
                url: "https://github.com/example/repo".to_string(),
                branch: None,
            }),
            mount_as: None,
        };
        assert!(src.is_remote_git());
    }
}
