use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use std::path::{Path, PathBuf};

mod doctor;
mod wizard;
mod workflow;
mod workspace_discovery;

use vwsd::git_ops;
use vwsd::git_source;
use vwsd::model::Workspace;
use vwsd::mount_helper;

#[derive(Debug, Parser)]
#[command(name = "vwsd")]
#[command(about = "Prepare real git-backed workspace directories")]
#[command(version = env!("VWSD_VERSION"))]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Prepare workspace directories (clone/fetch remote sources, create git worktrees)
    Mount {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Mount all configured workspaces
        #[arg(long, default_value_t = false)]
        all: bool,
        /// Print per-source results as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Diagnose workspace configs, worktrees and caches
    Doctor {
        /// Only check this config file (default: all configured workspaces)
        #[arg(long)]
        config: Option<PathBuf>,
        /// Print the report as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Apply safe automatic fixes (same as `vwsd repair`) before reporting
        #[arg(long, default_value_t = false)]
        fix: bool,
    },
    /// Create a workspace config (interactive wizard, or non-interactive with --name/--source)
    Init {
        /// Workspace name
        #[arg(long)]
        name: Option<String>,
        /// Source: local directory path or git URL (repeatable)
        #[arg(long = "source")]
        sources: Vec<String>,
        /// Override the default mountpoint path
        #[arg(long)]
        mountpoint: Option<String>,
        /// Config template (interactive wizard only): basic/frontend/rust/maven/monorepo
        #[arg(long, default_value = "basic")]
        template: String,
        /// Also generate a .code-workspace file
        #[arg(long, default_value_t = false)]
        vscode: bool,
        /// Print the generated config as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Show workspace and per-source sync status
    List {
        /// Print the status report as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Fetch from remotes before computing ahead/behind counts
        #[arg(long, default_value_t = false)]
        fetch: bool,
    },
    /// Alias for `list`
    Status {
        /// Print the status report as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Fetch from remotes before computing ahead/behind counts
        #[arg(long, default_value_t = false)]
        fetch: bool,
    },
    /// Alias for `mount`
    Start {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Mount all configured workspaces
        #[arg(long, default_value_t = false)]
        all: bool,
        /// Print per-source results as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Alias for `umount`
    Stop {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Unmount all configured workspaces
        #[arg(long, default_value_t = false)]
        all: bool,
        /// Skip confirmation prompts
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Re-run the mount/refresh logic for a workspace
    Restart {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Restart all configured workspaces
        #[arg(long, default_value_t = false)]
        all: bool,
        /// Print per-source results as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Remove remote worktrees (local sources are left untouched)
    Umount {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Unmount all configured workspaces
        #[arg(long, default_value_t = false)]
        all: bool,
        /// Skip confirmation prompts
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Delete worktrees, config, mountpoint and unused caches (irreversible; local sources are kept)
    Destroy {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Skip confirmation prompts
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Generate a shell completion script
    Completion {
        /// Shell to generate completions for
        shell: Shell,
    },
    /// Apply safe repairs (create missing mountpoints, backfill workspace ids)
    Repair {
        /// Print the applied repairs as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Fetch and rebase sources that are behind their remote
    Sync {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Print per-source results as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Push committed changes to origin (preview without --yes)
    Push {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Actually push (without it, only reports what would be pushed)
        #[arg(long, default_value_t = false)]
        yes: bool,
        /// Print per-source results as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Generate a multi-root .code-workspace file for the workspace
    Vscode {
        /// Workspace name (looked up in ~/.vws/workspaces)
        #[arg(long)]
        name: Option<String>,
        /// Path to a workspace config JSON file
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeardownKind {
    Umount,
    Destroy,
}

fn mounted_repo_path(ws: &Workspace, src: &vwsd::model::Source) -> PathBuf {
    mount_helper::source_runtime_path(ws, src)
}

/// True when `child` equals `parent` or lives inside it. Paths are
/// canonicalized when possible to see through symlinks and `..` segments.
fn path_contains(parent: &Path, child: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(child).starts_with(canon(parent))
}

fn check_git_sync_before_teardown(ws: &Workspace, force: bool, kind: TeardownKind) -> Result<bool> {
    let destructive = kind == TeardownKind::Destroy;
    let verb = if destructive { "destroy" } else { "remove" };
    let confirm = |warning: String, prompt: &str, default: bool, cancel: &str| -> Result<bool> {
        eprintln!("{}", warning);
        if force {
            return Ok(true);
        }
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(default)
            .interact()?;
        if !confirmed {
            println!("{}", cancel);
        }
        Ok(confirmed)
    };

    for src in ws.sources.iter().filter(|src| src.is_remote_git()) {
        let path = mounted_repo_path(ws, src);
        if !path.exists() {
            continue;
        }
        let Some(branch) = git_ops::current_branch(&path)
            .ok()
            .filter(|branch| !branch.is_empty())
        else {
            continue;
        };
        if let Err(err) = git_ops::fetch_all(&path) {
            eprintln!(
                "warning: could not fetch remote updates for '{}': {}",
                src.mount_name(),
                err
            );
        }
        let Ok(status) = git_ops::check_branch_sync(&path, &branch) else {
            continue;
        };
        if status.uncommitted_changes > 0 {
            let (warning, prompt, default, cancel) = if destructive {
                (
                    format!(
                        "🚨 remote source '{}' has {} uncommitted change(s); destroying this workspace will permanently delete that worktree",
                        src.mount_name(), status.uncommitted_changes
                    ),
                    "Continue and permanently remove this worktree?",
                    false,
                    "Cancelled.",
                )
            } else {
                (
                    format!(
                        "⚠️  remote source '{}' has {} uncommitted change(s)",
                        src.mount_name(), status.uncommitted_changes
                    ),
                    "Continue removing the worktree? You can recreate it later, but uncommitted work in this worktree will be lost.",
                    false,
                    "Cancelled.",
                )
            };
            if !confirm(warning, prompt, default, cancel)? {
                return Ok(false);
            }
        }
        if status.unpushed_commits > 0 {
            let (warning, prompt, default, cancel) = if destructive {
                (
                    format!(
                        "🚨 remote source '{}' has {} unpushed commit(s); destroying this workspace will permanently delete them if they only exist here",
                        src.mount_name(), status.unpushed_commits
                    ),
                    "Continue and permanently remove this worktree?",
                    false,
                    "Cancelled.",
                )
            } else {
                (
                    format!(
                        "⚠️  remote source '{}' has {} unpushed commit(s)",
                        src.mount_name(), status.unpushed_commits
                    ),
                    "Continue removing the worktree? Recreating later will not restore these commits automatically.",
                    false,
                    "Cancelled.",
                )
            };
            if !confirm(warning, prompt, default, cancel)? {
                return Ok(false);
            }
        }
        if status.behind_commits > 0 {
            eprintln!(
                "ℹ️  remote source '{}' is behind origin/{} by {} commit(s) before {}",
                src.mount_name(),
                branch,
                status.behind_commits,
                verb
            );
        }
    }
    Ok(true)
}

fn load_workspace(name: Option<&str>, config: Option<&Path>) -> Result<(PathBuf, Workspace)> {
    workflow::open_config(name, config)
}

fn unmount_workspace(config_path: &Path, force: bool) -> Result<()> {
    let (_path, ws) = load_workspace(None, Some(config_path))?;
    if !check_git_sync_before_teardown(&ws, force, TeardownKind::Umount)? {
        return Ok(());
    }
    if !force {
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Remove remote worktrees for workspace '{}' ?",
                ws.name
            ))
            .default(false)
            .interact()?;
        if !confirmed {
            println!("Cancelled.");
            return Ok(());
        }
    }
    let removed = mount_helper::cleanup_worktrees(&ws)?;
    println!(
        "Workspace '{}' removed {} remote worktree(s). Local sources were left in place.",
        ws.name, removed
    );
    Ok(())
}

fn destroy_workspace(config_path: &Path, force: bool) -> Result<()> {
    let text = std::fs::read_to_string(config_path)?;
    let mut ws: Workspace = serde_json::from_str(&text)?;
    ws.expand_paths();
    ws.validate_mount_names()?;

    if !check_git_sync_before_teardown(&ws, force, TeardownKind::Destroy)? {
        return Ok(());
    }
    if !force {
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Destroy workspace '{}' ? This deletes remote worktrees, the workspace config, and the workspace mountpoint directory.",
                ws.name
            ))
            .default(false)
            .interact()?;
        if !confirmed {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let _ = mount_helper::cleanup_worktrees(&ws)?;
    for src in ws.sources.iter().filter(|src| src.is_remote_git()) {
        let Some(remote_git) = &src.remote_git else {
            continue;
        };
        if let Ok(isolated_bare) = git_source::isolated_bare_path(&remote_git.url, &ws.name, None) {
            if isolated_bare.exists() {
                std::fs::remove_dir_all(&isolated_bare)?;
            }
            let lock = isolated_bare.with_extension("lock");
            if lock.exists() {
                let _ = std::fs::remove_file(lock);
            }
        }
        mount_helper::cleanup_bare_repo_if_unused(&remote_git.url, &ws.name)?;
    }
    // Never delete the mountpoint if a local source lives inside it: local
    // sources are the user's own directories and must survive destroy.
    let mountpoint = Path::new(&ws.mountpoint);
    let local_inside = ws
        .sources
        .iter()
        .filter(|src| !src.is_remote_git())
        .any(|src| path_contains(mountpoint, Path::new(&src.path)));
    if local_inside {
        println!(
            "Mountpoint {} contains local source directories; leaving it in place.",
            mountpoint.display()
        );
    } else {
        mount_helper::cleanup_mountpoint(mountpoint)?;
    }
    std::fs::remove_file(config_path)?;
    println!(
        "Workspace '{}' destroyed. Local source directories were not deleted.",
        ws.name
    );
    Ok(())
}

fn print_op_results(results: &[workflow::OperationResult], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(results)?);
    } else {
        workflow::print_results(results);
    }
    Ok(())
}

/// Mount every configured workspace, continuing past individual failures so
/// one broken config cannot block the rest. Returns an error at the end if
/// any workspace failed.
fn mount_all(json: bool) -> Result<()> {
    let mut failures = 0usize;
    for (config_path, _) in workspace_discovery::list_configs() {
        match workflow::mount_workspace_by_config(&config_path) {
            Ok(results) => print_op_results(&results, json)?,
            Err(err) => {
                failures += 1;
                eprintln!("error mounting {}: {:#}", config_path.display(), err);
            }
        }
    }
    if failures > 0 {
        bail!("{} workspace(s) failed to mount", failures);
    }
    Ok(())
}

/// Unmount every configured workspace, continuing past individual failures.
fn unmount_all(force: bool) -> Result<()> {
    let mut failures = 0usize;
    for (config_path, _) in workspace_discovery::list_configs() {
        if let Err(err) = unmount_workspace(&config_path, force) {
            failures += 1;
            eprintln!("error unmounting {}: {:#}", config_path.display(), err);
        }
    }
    if failures > 0 {
        bail!("{} workspace(s) failed to unmount", failures);
    }
    Ok(())
}

fn main() -> Result<()> {
    run_sync(Cli::parse().cmd)
}

fn run_sync(cmd: Command) -> Result<()> {
    match cmd {
        Command::Mount {
            name,
            config,
            all,
            json,
        }
        | Command::Start {
            name,
            config,
            all,
            json,
        } => {
            if all {
                return mount_all(json);
            }
            let (config_path, _) = load_workspace(name.as_deref(), config.as_deref())?;
            let results = workflow::mount_workspace_by_config(&config_path)?;
            print_op_results(&results, json)
        }
        Command::Doctor { config, json, fix } => {
            if fix {
                let results = workflow::repair()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&results)?);
                } else {
                    workflow::print_results(&results);
                }
            }
            doctor::run(config.as_deref(), json)
        }
        Command::Init {
            name,
            sources,
            mountpoint,
            template,
            vscode,
            json,
        } => {
            if sources.is_empty() && name.is_none() {
                wizard::run_wizard()
            } else {
                let name = name.unwrap_or_else(|| "my-workspace".to_string());
                let template = workflow::InitTemplate::from_str(&template)?;
                workflow::create_workspace(name, sources, mountpoint, template, vscode, json)
            }
        }
        Command::List { json, fetch } | Command::Status { json, fetch } => {
            let report = workflow::collect_status(fetch);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                workflow::print_status(&report);
            }
            Ok(())
        }
        Command::Restart {
            name,
            config,
            all,
            json,
        } => {
            if all {
                return mount_all(json);
            }
            let (config_path, _) = load_workspace(name.as_deref(), config.as_deref())?;
            let results = workflow::mount_workspace_by_config(&config_path)?;
            print_op_results(&results, json)
        }
        Command::Umount {
            name,
            config,
            all,
            force,
        }
        | Command::Stop {
            name,
            config,
            all,
            force,
        } => {
            if all {
                return unmount_all(force);
            }
            let (config_path, _) = load_workspace(name.as_deref(), config.as_deref())?;
            unmount_workspace(&config_path, force)
        }
        Command::Destroy {
            name,
            config,
            force,
        } => {
            let (config_path, _) = load_workspace(name.as_deref(), config.as_deref())?;
            destroy_workspace(&config_path, force)
        }
        Command::Completion { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            generate(shell, &mut cmd, name, &mut std::io::stdout());
            Ok(())
        }
        Command::Repair { json } => {
            let results = workflow::repair()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                workflow::print_results(&results);
            }
            Ok(())
        }
        Command::Sync { name, config, json } => {
            let results = workflow::sync_workspace(name.as_deref(), config.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                workflow::print_results(&results);
            }
            Ok(())
        }
        Command::Push {
            name,
            config,
            yes,
            json,
        } => {
            let results = workflow::push_workspace(name.as_deref(), config.as_deref(), yes)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                workflow::print_results(&results);
            }
            Ok(())
        }
        Command::Vscode { name, config } => {
            let (_path, ws) = load_workspace(name.as_deref(), config.as_deref())?;
            let path = workflow::write_vscode_workspace(&ws)?;
            println!("VS Code workspace written to {}", path.display());
            Ok(())
        }
    }
}
