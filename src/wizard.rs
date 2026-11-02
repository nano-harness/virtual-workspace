use anyhow::Result;
use inquire::{Confirm, Select, Text};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use vwsd::model::{RemoteGit, Source, Workspace};
use vwsd::util::expand_tilde;

pub fn run_wizard() -> Result<()> {
    println!("=== Virtual Workspace (vws) wizard ===");

    let templates = vec![
        "💡 Frontend workspace example",
        "💡 Rust workspace example",
        "🛠️  Manual setup",
    ];
    let selected = Select::new("Choose a template or manual setup:", templates.clone()).prompt()?;

    let workspace = if selected == templates[0] || selected == templates[1] {
        let default_path = if selected == templates[0] {
            expand_tilde("~/Projects/frontend")
        } else {
            expand_tilde("~/Projects/rust")
        };
        let default_name = if selected == templates[0] {
            "frontend-workspace"
        } else {
            "rust-workspace"
        };
        let name = Text::new("Workspace name:")
            .with_default(default_name)
            .prompt()?;
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let default_mp = format!("{}/VirtualWorkspaces/{}", home.to_string_lossy(), name);
        let mountpoint = expand_tilde(
            &Text::new("Workspace mountpoint:")
                .with_default(&default_mp)
                .prompt()?,
        );
        let local_path = expand_tilde(
            &Text::new("Local source path:")
                .with_default(&default_path)
                .prompt()?,
        );
        if !Path::new(&local_path).exists()
            && Confirm::new(&format!("'{}' does not exist. Create it?", local_path))
                .with_default(true)
                .prompt()?
        {
            fs::create_dir_all(&local_path)?;
        }
        Workspace {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            mountpoint,
            sources: vec![Source {
                path: local_path,
                remote_git: None,
                mount_as: None,
            }],
        }
    } else {
        run_manual_wizard()?
    };

    let warnings = audit_workspace(&workspace);
    if !warnings.is_empty() {
        println!("\nWarnings:");
        for warning in &warnings {
            println!("  - {}", warning);
        }
        if !Confirm::new("Save this config anyway?")
            .with_default(true)
            .prompt()?
        {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let config_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".vws")
        .join("workspaces");
    fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join(format!("{}.json", workspace.name));
    fs::write(&config_path, serde_json::to_string_pretty(&workspace)?)?;
    println!("\nConfig saved to: {}", config_path.display());
    println!("Prepare workspace with:");
    println!("  vwsd mount --config {}", config_path.display());
    Ok(())
}

fn run_manual_wizard() -> Result<Workspace> {
    let name = Text::new("Workspace name:")
        .with_default("my-workspace")
        .prompt()?;
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let default_mp = format!("{}/VirtualWorkspaces/{}", home.to_string_lossy(), name);
    let mountpoint = expand_tilde(
        &Text::new("Workspace mountpoint:")
            .with_default(&default_mp)
            .prompt()?,
    );

    let mut sources = Vec::new();
    loop {
        let prompt = if sources.is_empty() {
            "Add a source (local path or remote git URL):"
        } else {
            "Add another source (leave empty to finish):"
        };
        let input = Text::new(prompt)
            .with_help_message("Examples: ~/code/app or https://github.com/user/repo.git")
            .prompt()?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            if sources.is_empty() {
                println!("At least one source is required.");
                continue;
            }
            break;
        }

        if vwsd::url_parser::parse_git_url(trimmed).is_some() {
            let branch = Text::new("Branch (leave empty for remote default):")
                .with_default("")
                .prompt()?;
            let mount_as = Text::new("mount_as directory name inside the mountpoint (optional):")
                .with_default("")
                .prompt()?;
            sources.push(Source {
                path: String::new(),
                remote_git: Some(RemoteGit {
                    url: trimmed.to_string(),
                    branch: (!branch.trim().is_empty()).then(|| branch.trim().to_string()),
                }),
                mount_as: (!mount_as.trim().is_empty()).then(|| mount_as.trim().to_string()),
            });
        } else {
            let path = expand_tilde(trimmed);
            if !Path::new(&path).exists()
                && Confirm::new(&format!("'{}' does not exist. Create it?", path))
                    .with_default(false)
                    .prompt()?
            {
                fs::create_dir_all(&path)?;
            }
            let mount_as = Text::new("Display name for this local source (optional):")
                .with_default("")
                .prompt()?;
            sources.push(Source {
                path,
                remote_git: None,
                mount_as: (!mount_as.trim().is_empty()).then(|| mount_as.trim().to_string()),
            });
        }

        if !Confirm::new("Add another source?")
            .with_default(true)
            .prompt()?
        {
            break;
        }
    }

    Ok(Workspace {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        mountpoint,
        sources,
    })
}

pub fn audit_workspace(ws: &Workspace) -> Vec<String> {
    let mut warnings = Vec::new();
    if ws.sources.is_empty() {
        warnings.push("workspace has no sources".to_string());
        return warnings;
    }

    let mut local_paths = HashSet::new();
    let mut remote_mounts = HashSet::new();
    for src in &ws.sources {
        if src.is_remote_git() {
            let name = src.mount_name();
            if !remote_mounts.insert(name.clone()) {
                warnings.push(format!("duplicate remote mount name: {}", name));
            }
        } else {
            if !local_paths.insert(src.path.clone()) {
                warnings.push(format!("duplicate local source path: {}", src.path));
            }
            if ws.mountpoint == src.path {
                warnings.push(format!(
                    "mountpoint '{}' is the same as local source '{}'; local sources are used in place, not copied",
                    ws.mountpoint, src.path
                ));
            }
        }
    }
    warnings
}
