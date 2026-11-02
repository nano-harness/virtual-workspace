use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use vwsd::git_ops;
use vwsd::git_source;
use vwsd::model::{RemoteGit, Source, Workspace};
use vwsd::mount_helper;
use vwsd::util::expand_tilde;

use crate::workspace_discovery;

#[derive(Debug, Clone, Serialize)]
pub struct SourceStatusView {
    pub source: String,
    pub kind: String,
    pub path: String,
    pub branch: Option<String>,
    pub exists: bool,
    pub uncommitted_changes: Option<usize>,
    pub unpushed_commits: Option<usize>,
    pub behind_commits: Option<usize>,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceStatusView {
    pub name: String,
    pub config_path: String,
    pub mountpoint: String,
    pub ready: bool,
    pub sources: Vec<SourceStatusView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub workspaces: Vec<WorkspaceStatusView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationResult {
    pub workspace: String,
    pub source: Option<String>,
    pub action: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy)]
pub enum InitTemplate {
    Basic,
    Frontend,
    Rust,
    Maven,
    Monorepo,
}

impl InitTemplate {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "basic" => Ok(Self::Basic),
            "frontend" => Ok(Self::Frontend),
            "rust" => Ok(Self::Rust),
            "maven" => Ok(Self::Maven),
            "monorepo" => Ok(Self::Monorepo),
            other => anyhow::bail!(
                "unknown template '{}'; expected basic/frontend/rust/maven/monorepo",
                other
            ),
        }
    }
}

enum LocalMountOutcome {
    Ready(PathBuf),
    Initialized(PathBuf),
}

fn persist_workspace_config(config_path: &Path, workspace: &Workspace) -> Result<()> {
    // Write-then-rename so a crash or concurrent mount cannot leave a
    // truncated config behind.
    let mut tmp_path = config_path.to_path_buf();
    tmp_path.set_extension("json.tmp");
    fs::write(&tmp_path, serde_json::to_string_pretty(workspace)?)?;
    fs::rename(&tmp_path, config_path)?;
    Ok(())
}

fn source_display_name(src: &Source) -> String {
    src.mount_name()
}

fn is_git_repo(path: &Path) -> bool {
    git_ops::run_git(&["rev-parse", "--git-dir"], path).is_ok()
}

fn has_origin_remote(path: &Path) -> bool {
    git_ops::run_git(&["remote", "get-url", "origin"], path).is_ok()
}

fn ensure_local_git_source(src: &Source) -> Result<LocalMountOutcome> {
    let path = PathBuf::from(&src.path);
    if src.path.trim().is_empty() {
        anyhow::bail!("local source path is empty");
    }
    if !path.exists() {
        anyhow::bail!("local source path '{}' does not exist", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("local source path '{}' is not a directory", path.display());
    }
    if path.join(".git").exists() || is_git_repo(&path) {
        return Ok(LocalMountOutcome::Ready(path));
    }
    git_ops::run_git(&["init"], &path)
        .with_context(|| format!("failed to run git init in {}", path.display()))?;
    Ok(LocalMountOutcome::Initialized(path))
}

pub fn mount_source(ws: &Workspace, src: &mut Source) -> Result<PathBuf> {
    if src.is_remote_git() {
        let target = mount_helper::remote_target_path(ws, src);
        let (path, _) = git_source::prepare_remote_source(src, &ws.name, &target, None)?;
        Ok(path)
    } else {
        match ensure_local_git_source(src)? {
            LocalMountOutcome::Ready(path) | LocalMountOutcome::Initialized(path) => Ok(path),
        }
    }
}

fn duplicate_remote_mounts(ws: &Workspace) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut duplicates = Vec::new();
    for src in ws.sources.iter().filter(|src| src.is_remote_git()) {
        let name = src.mount_name();
        if !seen.insert(name.clone()) {
            duplicates.push(name);
        }
    }
    duplicates.sort();
    duplicates.dedup();
    duplicates
}

pub fn open_config(name: Option<&str>, config: Option<&Path>) -> Result<(PathBuf, Workspace)> {
    let (path, ws) = if let Some(path) = config {
        let text = fs::read_to_string(path)?;
        let mut ws: Workspace = serde_json::from_str(&text)?;
        ws.expand_paths();
        (path.to_path_buf(), ws)
    } else if let Some(name) = name {
        workspace_discovery::find_config_by_name(name)
            .with_context(|| format!("workspace '{}' not found", name))?
    } else {
        workspace_discovery::select_workspace("Select workspace")?
            .context("no workspace selected")?
    };
    ws.validate_mount_names()?;
    Ok((path, ws))
}

pub fn mount_workspace_by_config(config_path: &Path) -> Result<Vec<OperationResult>> {
    let text = fs::read_to_string(config_path)?;
    let mut ws: Workspace = serde_json::from_str(&text)?;
    ws.expand_paths();
    ws.validate_mount_names()?;
    let mut original: Workspace = serde_json::from_str(&text)?;
    let workspace_name = ws.name.clone();
    let workspace_view = Workspace {
        id: ws.id.clone(),
        name: workspace_name.clone(),
        mountpoint: ws.mountpoint.clone(),
        sources: Vec::new(),
    };

    let mut results = Vec::new();
    let mut changed = false;
    if original.ensure_id() {
        ws.id = original.id.clone();
        changed = true;
    }

    let duplicates = duplicate_remote_mounts(&ws);
    if !duplicates.is_empty() {
        anyhow::bail!(
            "workspace '{}' has duplicate remote mount names: {}",
            ws.name,
            duplicates.join(", ")
        );
    }

    fs::create_dir_all(&ws.mountpoint)?;

    // Mount sources in parallel: each remote source pays a fetch plus several
    // git spawns, and per-bare file locks already serialize same-repo access.
    // Handles are joined in spawn order, so result ordering stays stable.
    let outcomes: Vec<Result<(OperationResult, bool)>> = std::thread::scope(|scope| {
        let ws_name = &ws.name;
        let view = &workspace_view;
        let handles: Vec<_> = ws
            .sources
            .iter_mut()
            .zip(original.sources.iter_mut())
            .map(|(src, original_src)| {
                scope.spawn(move || {
                    let source_name = source_display_name(src);
                    if src.is_remote_git() {
                        let path = mount_source(view, src).with_context(|| {
                            format!("failed to mount remote source '{}'", source_name)
                        })?;
                        let branch_changed = src
                            .remote_git
                            .as_ref()
                            .and_then(|src_rg| {
                                original_src
                                    .remote_git
                                    .as_ref()
                                    .map(|orig_rg| src_rg.branch != orig_rg.branch)
                            })
                            .unwrap_or(false);
                        let mut src_changed = false;
                        if branch_changed || src.mount_as != original_src.mount_as {
                            original_src.mount_as = src.mount_as.clone();
                            if let (Some(orig_rg), Some(src_rg)) =
                                (&mut original_src.remote_git, &src.remote_git)
                            {
                                orig_rg.branch = src_rg.branch.clone();
                            }
                            src_changed = true;
                        }
                        Ok((
                            OperationResult {
                                workspace: ws_name.clone(),
                                source: Some(source_name),
                                action: "mount".to_string(),
                                status: "ok".to_string(),
                                message: format!("remote worktree ready at {}", path.display()),
                            },
                            src_changed,
                        ))
                    } else {
                        let result = match ensure_local_git_source(src) {
                            Ok(LocalMountOutcome::Ready(path)) => OperationResult {
                                workspace: ws_name.clone(),
                                source: Some(source_name),
                                action: "mount".to_string(),
                                status: "ok".to_string(),
                                message: format!(
                                    "using local repository in place at {}",
                                    path.display()
                                ),
                            },
                            Ok(LocalMountOutcome::Initialized(path)) => OperationResult {
                                workspace: ws_name.clone(),
                                source: Some(source_name),
                                action: "mount".to_string(),
                                status: "ok".to_string(),
                                message: format!(
                                    "initialized local repository in place at {}",
                                    path.display()
                                ),
                            },
                            Err(err) => OperationResult {
                                workspace: ws_name.clone(),
                                source: Some(source_name),
                                action: "mount".to_string(),
                                status: "warn".to_string(),
                                message: err.to_string(),
                            },
                        };
                        Ok((result, false))
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("mount worker panicked"))
            .collect()
    });
    for outcome in outcomes {
        let (result, src_changed) = outcome?;
        changed |= src_changed;
        results.push(result);
    }

    if changed {
        persist_workspace_config(config_path, &original)?;
        results.push(OperationResult {
            workspace: workspace_name,
            source: None,
            action: "save_config".to_string(),
            status: "ok".to_string(),
            message: config_path.display().to_string(),
        });
    }

    Ok(results)
}

fn branch_for_source(src: &Source, path: &Path) -> Option<String> {
    git_ops::current_branch(path)
        .ok()
        .filter(|branch| !branch.is_empty())
        .or_else(|| src.remote_git.as_ref().and_then(|rg| rg.branch.clone()))
}

fn status_for_source(ws: &Workspace, src: &Source, fetch: bool) -> SourceStatusView {
    let path = mount_helper::source_runtime_path(ws, src);
    let source = source_display_name(src);
    let kind = if src.is_remote_git() {
        "remote"
    } else {
        "local"
    }
    .to_string();
    if !path.exists() {
        return SourceStatusView {
            source,
            kind,
            path: path.display().to_string(),
            branch: src.remote_git.as_ref().and_then(|rg| rg.branch.clone()),
            exists: false,
            uncommitted_changes: None,
            unpushed_commits: None,
            behind_commits: None,
            state: "missing".to_string(),
            error: None,
        };
    }
    if !is_git_repo(&path) {
        return SourceStatusView {
            source,
            kind,
            path: path.display().to_string(),
            branch: None,
            exists: true,
            uncommitted_changes: None,
            unpushed_commits: None,
            behind_commits: None,
            state: "not_git".to_string(),
            error: Some("path exists but is not a git repository".to_string()),
        };
    }
    if fetch && has_origin_remote(&path) {
        let _ = git_ops::fetch_all(&path);
    }
    let branch = branch_for_source(src, &path);
    let sync = branch
        .as_deref()
        .and_then(|branch| git_ops::check_branch_sync(&path, branch).ok());
    // check_branch_sync already counts uncommitted changes via
    // `status --porcelain=v2`; only pay for a second status spawn when it
    // failed (e.g. branch undetermined).
    let uncommitted = if sync.is_none() {
        git_ops::count_uncommitted_changes(&path).ok()
    } else {
        None
    };
    let state = if let Some(sync) = &sync {
        if sync.is_fully_synced() {
            "ready"
        } else if sync.uncommitted_changes > 0 {
            "dirty"
        } else if sync.unpushed_commits > 0 {
            "unpushed"
        } else if sync.behind_commits > 0 {
            "behind"
        } else {
            "git"
        }
    } else if let Some(count) = uncommitted {
        if count > 0 {
            "dirty"
        } else {
            "git"
        }
    } else {
        "git"
    };

    SourceStatusView {
        source,
        kind,
        path: path.display().to_string(),
        branch,
        exists: true,
        uncommitted_changes: sync.as_ref().map(|s| s.uncommitted_changes).or(uncommitted),
        unpushed_commits: sync.as_ref().map(|s| s.unpushed_commits),
        behind_commits: sync.as_ref().map(|s| s.behind_commits),
        state: state.to_string(),
        error: None,
    }
}

pub fn collect_status(fetch_git: bool) -> StatusReport {
    let configs = workspace_discovery::list_configs();
    // Compute per-source statuses in parallel: each one costs several git
    // spawns (plus a fetch with --fetch), so serial collection scales poorly
    // with the number of workspaces and sources. Handles are joined in spawn
    // order to keep output ordering stable.
    let grouped: Vec<Vec<SourceStatusView>> = std::thread::scope(|scope| {
        let handles: Vec<_> = configs
            .iter()
            .enumerate()
            .flat_map(|(wi, (_, workspace))| {
                workspace
                    .sources
                    .iter()
                    .map(move |src| (wi, workspace, src))
            })
            .map(|(wi, workspace, src)| {
                scope.spawn(move || (wi, status_for_source(workspace, src, fetch_git)))
            })
            .collect();
        let mut grouped: Vec<Vec<SourceStatusView>> = configs.iter().map(|_| Vec::new()).collect();
        for handle in handles {
            let (wi, status) = handle.join().expect("status worker panicked");
            grouped[wi].push(status);
        }
        grouped
    });
    let workspaces = configs
        .into_iter()
        .zip(grouped)
        .map(|((config_path, workspace), sources)| {
            let ready = sources
                .iter()
                .all(|source| source.exists && source.state != "not_git");
            WorkspaceStatusView {
                name: workspace.name.clone(),
                config_path: config_path.display().to_string(),
                mountpoint: workspace.mountpoint.clone(),
                ready,
                sources,
            }
        })
        .collect();
    StatusReport { workspaces }
}

/// Pad to `width` chars, truncating with an ellipsis when longer, so long
/// mount names cannot break the status table's column alignment.
fn pad_trunc(value: &str, width: usize) -> String {
    if value.chars().count() > width {
        let truncated: String = value.chars().take(width.saturating_sub(1)).collect();
        format!("{:<width$}", format!("{}…", truncated), width = width)
    } else {
        format!("{:<width$}", value, width = width)
    }
}

pub fn print_status(report: &StatusReport) {
    if report.workspaces.is_empty() {
        println!("No workspaces found. Use `vwsd init` to create one.");
        return;
    }
    for ws in &report.workspaces {
        println!(
            "{}  {}  {}",
            ws.name,
            if ws.ready { "ready" } else { "needs-attention" },
            ws.mountpoint
        );
        for source in &ws.sources {
            println!(
                "  {} {} {} {}",
                pad_trunc(&source.kind, 8),
                pad_trunc(&source.source, 16),
                pad_trunc(&source.state, 10),
                source.path
            );
            if let Some(branch) = &source.branch {
                println!("    branch: {}", branch);
            }
            if let Some(error) = &source.error {
                println!("    error: {}", error);
            }
        }
    }
}

pub fn repair() -> Result<Vec<OperationResult>> {
    let mut results = Vec::new();
    for (config_path, ws) in workspace_discovery::list_configs() {
        let text = fs::read_to_string(&config_path)?;
        let mut original: Workspace = serde_json::from_str(&text)?;
        let mut changed = false;
        if original.ensure_id() {
            changed = true;
            results.push(OperationResult {
                workspace: original.name.clone(),
                source: None,
                action: "assign_id".to_string(),
                status: "ok".to_string(),
                message: original.id.clone(),
            });
        }
        if let Some(parent) = Path::new(&ws.mountpoint).parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
                results.push(OperationResult {
                    workspace: ws.name.clone(),
                    source: None,
                    action: "create_mountpoint_parent".to_string(),
                    status: "ok".to_string(),
                    message: parent.display().to_string(),
                });
            }
        }
        if !Path::new(&ws.mountpoint).exists() {
            fs::create_dir_all(&ws.mountpoint)?;
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: None,
                action: "create_mountpoint".to_string(),
                status: "ok".to_string(),
                message: ws.mountpoint.clone(),
            });
        }
        if changed {
            persist_workspace_config(&config_path, &original)?;
        }
    }
    Ok(results)
}

pub fn print_results(results: &[OperationResult]) {
    for result in results {
        if let Some(source) = &result.source {
            println!(
                "{}:{} [{}] {} - {}",
                result.workspace, source, result.status, result.action, result.message
            );
        } else {
            println!(
                "{} [{}] {} - {}",
                result.workspace, result.status, result.action, result.message
            );
        }
    }
}

pub fn create_workspace(
    name: String,
    sources: Vec<String>,
    mountpoint: Option<String>,
    template: InitTemplate,
    vscode: bool,
    print_json: bool,
) -> Result<()> {
    anyhow::ensure!(
        !sources.is_empty(),
        "--source is required for non-interactive init"
    );
    if !matches!(template, InitTemplate::Basic) {
        eprintln!(
            "note: --template only applies to the interactive wizard; ignoring it for non-interactive init"
        );
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mountpoint = mountpoint
        .map(|path| expand_tilde(&path))
        .unwrap_or_else(|| format!("{}/VirtualWorkspaces/{}", home.to_string_lossy(), name));
    let mut parsed_sources = Vec::new();
    for raw in sources {
        let trimmed = raw.trim();
        if vwsd::url_parser::parse_git_url(trimmed).is_some()
            || Path::new(trimmed).extension().and_then(|e| e.to_str()) == Some("git")
        {
            parsed_sources.push(Source {
                path: String::new(),
                remote_git: Some(RemoteGit {
                    url: trimmed.to_string(),
                    branch: None,
                }),
                mount_as: None,
            });
        } else {
            let path = expand_tilde(trimmed);
            fs::create_dir_all(&path)
                .with_context(|| format!("failed to create source {}", path))?;
            parsed_sources.push(Source {
                path,
                remote_git: None,
                mount_as: None,
            });
        }
    }
    let ws = Workspace {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.clone(),
        mountpoint,
        sources: parsed_sources,
    };
    let config_dir = workspace_discovery::config_dir();
    fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join(format!("{}.json", name));
    persist_workspace_config(&config_path, &ws)?;
    if vscode {
        write_vscode_workspace(&ws)?;
    }
    if print_json {
        println!("{}", serde_json::to_string_pretty(&ws)?);
    } else {
        println!("Workspace config saved: {}", config_path.display());
        println!(
            "Prepare with: vwsd mount --config {}",
            config_path.display()
        );
    }
    Ok(())
}

pub fn write_vscode_workspace(ws: &Workspace) -> Result<PathBuf> {
    #[derive(Serialize)]
    struct Folder {
        path: String,
        name: String,
    }
    #[derive(Serialize)]
    struct VsCodeWorkspace {
        folders: Vec<Folder>,
        settings: serde_json::Value,
    }

    fs::create_dir_all(&ws.mountpoint)?;
    let path = PathBuf::from(&ws.mountpoint).join(format!("{}.code-workspace", ws.name));
    let folders = ws
        .sources
        .iter()
        .map(|source| Folder {
            path: mount_helper::source_runtime_path(ws, source)
                .display()
                .to_string(),
            name: source_display_name(source),
        })
        .collect();
    let doc = VsCodeWorkspace {
        folders,
        settings: serde_json::json!({}),
    };
    fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
    Ok(path)
}

pub fn sync_workspace(name: Option<&str>, config: Option<&Path>) -> Result<Vec<OperationResult>> {
    let (_path, ws) = open_config(name, config)?;
    let mut results = Vec::new();
    for src in &ws.sources {
        let source = source_display_name(src);
        let path = mount_helper::source_runtime_path(&ws, src);
        if !path.exists() {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "sync".to_string(),
                status: "skipped".to_string(),
                message: "repository path missing; run mount first".to_string(),
            });
            continue;
        }
        if !is_git_repo(&path) {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "sync".to_string(),
                status: "skipped".to_string(),
                message: "not a git repository".to_string(),
            });
            continue;
        }
        if !has_origin_remote(&path) {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "sync".to_string(),
                status: "skipped".to_string(),
                message: "origin remote not configured".to_string(),
            });
            continue;
        }
        git_ops::fetch_all(&path)?;
        let Some(branch) = branch_for_source(src, &path) else {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "sync".to_string(),
                status: "skipped".to_string(),
                message: "cannot determine current branch".to_string(),
            });
            continue;
        };
        let status = git_ops::check_branch_sync(&path, &branch)?;
        if status.uncommitted_changes > 0 || status.unpushed_commits > 0 {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "sync".to_string(),
                status: "skipped".to_string(),
                message: format!(
                    "dirty:{} unpushed:{}",
                    status.uncommitted_changes, status.unpushed_commits
                ),
            });
        } else if status.behind_commits > 0 {
            git_ops::rebase_onto_tracking(&path, &branch)?;
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "sync".to_string(),
                status: "ok".to_string(),
                message: format!(
                    "rebased {} commit(s) from origin/{}",
                    status.behind_commits, branch
                ),
            });
        } else {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "sync".to_string(),
                status: "ok".to_string(),
                message: "already up to date".to_string(),
            });
        }
    }
    Ok(results)
}

pub fn push_workspace(
    name: Option<&str>,
    config: Option<&Path>,
    yes: bool,
) -> Result<Vec<OperationResult>> {
    let (_path, ws) = open_config(name, config)?;
    let mut results = Vec::new();
    for src in &ws.sources {
        let source = source_display_name(src);
        let path = mount_helper::source_runtime_path(&ws, src);
        if !path.exists() || !is_git_repo(&path) {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "push".to_string(),
                status: "skipped".to_string(),
                message: "repository missing".to_string(),
            });
            continue;
        }
        if !has_origin_remote(&path) {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "push".to_string(),
                status: "skipped".to_string(),
                message: "origin remote not configured".to_string(),
            });
            continue;
        }
        let Some(branch) = branch_for_source(src, &path) else {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "push".to_string(),
                status: "skipped".to_string(),
                message: "cannot determine current branch".to_string(),
            });
            continue;
        };
        let status = git_ops::check_branch_sync(&path, &branch)?;
        if status.uncommitted_changes > 0 {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "push".to_string(),
                status: "skipped".to_string(),
                message: format!(
                    "{} uncommitted change(s); commit first",
                    status.uncommitted_changes
                ),
            });
        } else if status.unpushed_commits == 0 {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "push".to_string(),
                status: "ok".to_string(),
                message: "nothing to push".to_string(),
            });
        } else if yes {
            git_ops::push_to_branch(&path, &branch)?;
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "push".to_string(),
                status: "ok".to_string(),
                message: format!("pushed {} commit(s) to {}", status.unpushed_commits, branch),
            });
        } else {
            results.push(OperationResult {
                workspace: ws.name.clone(),
                source: Some(source),
                action: "push".to_string(),
                status: "pending".to_string(),
                message: format!(
                    "{} commit(s) ready; rerun with --yes",
                    status.unpushed_commits
                ),
            });
        }
    }
    Ok(results)
}
