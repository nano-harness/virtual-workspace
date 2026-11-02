use anyhow::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use vwsd::git_source;
use vwsd::model::Workspace;
use vwsd::mount_helper;

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub environment: Vec<CheckResult>,
    pub workspaces: Vec<WorkspaceReport>,
    pub summary: Summary,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceReport {
    pub name: String,
    pub config_path: String,
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub group: String,
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    pub fix_hint: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub total_workspaces: usize,
    pub passed: usize,
    pub warnings: usize,
    pub failures: usize,
}

fn check_pass(group: &str, name: &str, message: impl Into<String>) -> CheckResult {
    CheckResult {
        group: group.to_string(),
        name: name.to_string(),
        status: CheckStatus::Pass,
        message: message.into(),
        fix_hint: None,
    }
}

fn check_warn(
    group: &str,
    name: &str,
    message: impl Into<String>,
    fix_hint: Option<String>,
) -> CheckResult {
    CheckResult {
        group: group.to_string(),
        name: name.to_string(),
        status: CheckStatus::Warn,
        message: message.into(),
        fix_hint,
    }
}

fn check_fail(
    group: &str,
    name: &str,
    message: impl Into<String>,
    fix_hint: Option<String>,
) -> CheckResult {
    CheckResult {
        group: group.to_string(),
        name: name.to_string(),
        status: CheckStatus::Fail,
        message: message.into(),
        fix_hint,
    }
}

fn is_git_repo(path: &Path) -> bool {
    vwsd::git_ops::run_git(&["rev-parse", "--git-dir"], path).is_ok()
}

fn check_environment() -> Vec<CheckResult> {
    match std::process::Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            vec![check_pass(
                "environment",
                "git_available",
                format!("git available: {}", version),
            )]
        }
        Ok(output) => vec![check_fail(
            "environment",
            "git_available",
            format!(
                "git --version failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Some("Reinstall git".to_string()),
        )],
        Err(err) => vec![check_fail(
            "environment",
            "git_available",
            format!("git binary not found on PATH: {}", err),
            Some("Install git and ensure it is on PATH".to_string()),
        )],
    }
}

fn check_config(ws: &Workspace) -> Vec<CheckResult> {
    let mut checks = Vec::new();
    let mut missing = Vec::new();
    if ws.name.trim().is_empty() {
        missing.push("name");
    }
    if ws.mountpoint.trim().is_empty() {
        missing.push("mountpoint");
    }
    if ws.sources.is_empty() {
        missing.push("sources");
    }
    if missing.is_empty() {
        checks.push(check_pass(
            "config",
            "required_fields",
            "Required fields present",
        ));
    } else {
        checks.push(check_fail(
            "config",
            "required_fields",
            format!("Missing required fields: {}", missing.join(", ")),
            None,
        ));
    }

    if let Some(parent) = Path::new(&ws.mountpoint).parent() {
        if parent.exists() || parent == Path::new("") {
            checks.push(check_pass(
                "config",
                "mountpoint_parent",
                format!("Mountpoint parent exists: {}", parent.display()),
            ));
        } else {
            checks.push(check_fail(
                "config",
                "mountpoint_parent",
                format!("Mountpoint parent missing: {}", parent.display()),
                Some(format!("mkdir -p '{}'", parent.display())),
            ));
        }
    }

    let mut remote_names = HashSet::new();
    let mut duplicate_names = Vec::new();
    for src in ws.sources.iter().filter(|src| src.is_remote_git()) {
        let name = src.mount_name();
        if !remote_names.insert(name.clone()) {
            duplicate_names.push(name);
        }
    }
    if duplicate_names.is_empty() {
        checks.push(check_pass(
            "config",
            "remote_mount_names",
            "Remote mount names are unique",
        ));
    } else {
        duplicate_names.sort();
        duplicate_names.dedup();
        checks.push(check_fail(
            "config",
            "remote_mount_names",
            format!(
                "Duplicate remote mount names: {}",
                duplicate_names.join(", ")
            ),
            Some("Set distinct mount_as values for remote sources".to_string()),
        ));
    }

    checks
}

fn check_sources(ws: &Workspace) -> Vec<CheckResult> {
    let mut checks = Vec::new();
    for src in &ws.sources {
        let name = src.mount_name();
        if let Some(remote_git) = &src.remote_git {
            let target = mount_helper::remote_target_path(ws, src);
            if target.exists() {
                if target.join(".git").is_file() {
                    checks.push(check_pass(
                        "git",
                        &format!("{}:worktree", name),
                        format!("Remote worktree exists at {}", target.display()),
                    ));
                } else {
                    checks.push(check_fail(
                        "git",
                        &format!("{}:worktree", name),
                        format!("{} exists but is not a git worktree", target.display()),
                        Some(format!(
                            "Remove '{}' and re-run vwsd mount",
                            target.display()
                        )),
                    ));
                }
            } else {
                checks.push(check_warn(
                    "git",
                    &format!("{}:worktree", name),
                    format!("Remote worktree missing at {}", target.display()),
                    Some("Run vwsd mount to create the worktree".to_string()),
                ));
            }

            match git_source::shared_bare_path(&remote_git.url, None) {
                Ok(path) if path.exists() => checks.push(check_pass(
                    "git",
                    &format!("{}:shared_bare", name),
                    format!("Shared bare cache present at {}", path.display()),
                )),
                Ok(path) => checks.push(check_warn(
                    "git",
                    &format!("{}:shared_bare", name),
                    format!("Shared bare cache missing at {}", path.display()),
                    Some("Run vwsd mount to populate the cache".to_string()),
                )),
                Err(err) => checks.push(check_fail(
                    "git",
                    &format!("{}:shared_bare", name),
                    err.to_string(),
                    None,
                )),
            }

            match git_source::isolated_bare_path(&remote_git.url, &ws.name, None) {
                Ok(path) if path.exists() => checks.push(check_pass(
                    "git",
                    &format!("{}:isolated_bare", name),
                    format!("Workspace bare cache present at {}", path.display()),
                )),
                Ok(path) => checks.push(check_warn(
                    "git",
                    &format!("{}:isolated_bare", name),
                    format!("Workspace bare cache missing at {}", path.display()),
                    Some("Run vwsd mount to populate the cache".to_string()),
                )),
                Err(err) => checks.push(check_fail(
                    "git",
                    &format!("{}:isolated_bare", name),
                    err.to_string(),
                    None,
                )),
            }
        } else {
            let path = PathBuf::from(&src.path);
            if !path.exists() {
                checks.push(check_warn(
                    "local",
                    &format!("{}:path", name),
                    format!("Local source path missing: {}", path.display()),
                    Some("Fix the path or create the directory before mounting".to_string()),
                ));
                continue;
            }
            if !path.is_dir() {
                checks.push(check_fail(
                    "local",
                    &format!("{}:path", name),
                    format!("Local source is not a directory: {}", path.display()),
                    None,
                ));
                continue;
            }
            checks.push(check_pass(
                "local",
                &format!("{}:path", name),
                format!("Local source exists: {}", path.display()),
            ));
            if is_git_repo(&path) || path.join(".git").exists() {
                checks.push(check_pass(
                    "local",
                    &format!("{}:git", name),
                    format!("Local source is a git repository: {}", path.display()),
                ));
            } else {
                checks.push(check_warn(
                    "local",
                    &format!("{}:git", name),
                    format!(
                        "Local source is not yet a git repository; mount will run `git init` in {}",
                        path.display()
                    ),
                    Some(format!("git init '{}'", path.display())),
                ));
            }
        }
    }
    checks
}

fn check_workspace(config_path: &Path) -> WorkspaceReport {
    let fallback_name = config_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut checks = Vec::new();
    let text = match std::fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(err) => {
            checks.push(check_fail("config", "file_readable", err.to_string(), None));
            return WorkspaceReport {
                name: fallback_name,
                config_path: config_path.display().to_string(),
                checks,
            };
        }
    };

    let ws: Workspace = match serde_json::from_str(&text) {
        Ok(workspace) => {
            checks.push(check_pass("config", "json_syntax", "JSON syntax valid"));
            workspace
        }
        Err(err) => {
            checks.push(check_fail(
                "config",
                "json_syntax",
                err.to_string(),
                Some("Fix the JSON syntax error in the config file".to_string()),
            ));
            return WorkspaceReport {
                name: fallback_name,
                config_path: config_path.display().to_string(),
                checks,
            };
        }
    };

    let mut ws_expanded = ws;
    ws_expanded.expand_paths();
    checks.extend(check_config(&ws_expanded));
    checks.extend(check_sources(&ws_expanded));

    WorkspaceReport {
        name: if ws_expanded.name.trim().is_empty() {
            fallback_name
        } else {
            ws_expanded.name.clone()
        },
        config_path: config_path.display().to_string(),
        checks,
    }
}

fn group_display_name(group: &str) -> &str {
    match group {
        "environment" => "Environment",
        "config" => "Config",
        "git" => "Remote Git",
        "local" => "Local Sources",
        other => other,
    }
}

fn print_check(check: &CheckResult) {
    let icon = match check.status {
        CheckStatus::Pass => "✓",
        CheckStatus::Warn => "⚠",
        CheckStatus::Fail => "✗",
    };
    println!("    {} {}", icon, check.message);
    if let Some(hint) = &check.fix_hint {
        println!("      Fix: {}", hint);
    }
}

fn print_report(report: &DoctorReport) {
    if !report.environment.is_empty() {
        println!("=== Environment ===");
        for check in &report.environment {
            print_check(check);
        }
        println!();
    }
    for ws_report in &report.workspaces {
        println!("=== Workspace: {} ===", ws_report.name);
        println!("  Config: {}", ws_report.config_path);

        let mut current_group = String::new();
        for check in &ws_report.checks {
            if check.group != current_group {
                current_group = check.group.clone();
                println!("\n  [{}]", group_display_name(&current_group));
            }
            print_check(check);
        }
        println!();
    }

    let summary = &report.summary;
    println!(
        "Summary: {} workspace(s), {} passed, {} warning(s), {} failure(s)",
        summary.total_workspaces, summary.passed, summary.warnings, summary.failures
    );
}

pub fn run(config: Option<&Path>, json: bool) -> Result<()> {
    let environment = check_environment();
    let config_paths: Vec<PathBuf> = if let Some(path) = config {
        vec![path.to_path_buf()]
    } else {
        let mut paths: Vec<PathBuf> = crate::workspace_discovery::list_configs()
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        paths.sort();
        paths
    };

    if config_paths.is_empty() {
        let (passed, failures) =
            environment
                .iter()
                .fold((0usize, 0usize), |(p, f), c| match c.status {
                    CheckStatus::Pass => (p + 1, f),
                    CheckStatus::Fail => (p, f + 1),
                    CheckStatus::Warn => (p, f),
                });
        let report = DoctorReport {
            environment,
            workspaces: vec![],
            summary: Summary {
                total_workspaces: 0,
                passed,
                warnings: 0,
                failures,
            },
        };
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print_report(&report);
            println!("No workspace config files found.");
        }
        return Ok(());
    }

    let workspaces: Vec<WorkspaceReport> = config_paths
        .iter()
        .map(|path| check_workspace(path))
        .collect();
    let mut passed = 0usize;
    let mut warnings = 0usize;
    let mut failures = 0usize;
    for check in environment
        .iter()
        .chain(workspaces.iter().flat_map(|ws| ws.checks.iter()))
    {
        match check.status {
            CheckStatus::Pass => passed += 1,
            CheckStatus::Warn => warnings += 1,
            CheckStatus::Fail => failures += 1,
        }
    }
    let total_workspaces = workspaces.len();
    let report = DoctorReport {
        environment,
        workspaces,
        summary: Summary {
            total_workspaces,
            passed,
            warnings,
            failures,
        },
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    Ok(())
}
