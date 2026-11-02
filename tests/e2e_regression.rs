use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn run_git(args: &[&str], cwd: &Path) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|err| panic!("failed to execute git {}: {}", args.join(" "), err));
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_bare_with_commit(root: &Path) -> PathBuf {
    let bare = root.join("origin.git");
    let seed = root.join("seed");
    run_git(&["init", "--bare", bare.to_str().unwrap()], root);
    run_git(
        &["clone", bare.to_str().unwrap(), seed.to_str().unwrap()],
        root,
    );
    run_git(&["config", "user.email", "test@test.com"], &seed);
    run_git(&["config", "user.name", "Test"], &seed);
    fs::write(seed.join("remote.txt"), "remote\n").unwrap();
    run_git(&["add", "-A"], &seed);
    run_git(&["commit", "-m", "init"], &seed);
    run_git(&["push"], &seed);
    bare
}

fn vwsd(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_vwsd"));
    cmd.env("HOME", home);
    cmd
}

#[test]
fn mixed_workspace_uses_local_repo_in_place_and_destroy_keeps_local_dir() {
    let td = TempDir::new().unwrap();
    let home = td.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let bare = init_bare_with_commit(td.path());
    let local = td.path().join("local-app");
    fs::create_dir_all(&local).unwrap();
    fs::write(local.join("main.rs"), "fn main() {}\n").unwrap();

    let mountpoint = td.path().join("workspace");
    let config_dir = home.join(".vws/workspaces");
    fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("mixed.json");
    let config = serde_json::json!({
        "name": "mixed",
        "mountpoint": mountpoint,
        "sources": [
            { "path": local, "mount_as": "local-app" },
            {
                "path": "",
                "mount_as": "remote-lib",
                "remote_git": { "url": bare, "branch": null }
            }
        ]
    });
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let output = vwsd(&home)
        .args(["mount", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(td.path().join("local-app/.git").exists());
    assert!(td.path().join("workspace/remote-lib/.git").is_file());

    let output = vwsd(&home).args(["status", "--json"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let sources = status["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["name"] == "mixed")
        .and_then(|workspace| workspace["sources"].as_array())
        .unwrap();
    assert!(sources.iter().any(|source| {
        source["kind"] == "local"
            && source["path"] == td.path().join("local-app").display().to_string()
    }));
    assert!(sources.iter().any(|source| {
        source["kind"] == "remote"
            && source["path"] == td.path().join("workspace/remote-lib").display().to_string()
    }));

    let output = vwsd(&home)
        .args(["vscode", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let vscode_path = td.path().join("workspace/mixed.code-workspace");
    let vscode = fs::read_to_string(vscode_path).unwrap();
    assert!(vscode.contains(&td.path().join("local-app").display().to_string()));
    assert!(vscode.contains(&td.path().join("workspace/remote-lib").display().to_string()));

    let output = vwsd(&home)
        .args([
            "doctor",
            "--config",
            config_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let doctor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doctor["summary"]["failures"], 0);

    let output = vwsd(&home)
        .args([
            "destroy",
            "--config",
            config_path.to_str().unwrap(),
            "--force",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(td.path().join("local-app").exists());
    assert!(!td.path().join("workspace/remote-lib").exists());
    assert!(!td.path().join("workspace").exists());
    assert!(!config_path.exists());
}
