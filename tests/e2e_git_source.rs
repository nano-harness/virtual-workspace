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
    fs::write(seed.join("lib.rs"), "pub fn hello() {}\n").unwrap();
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
fn mount_creates_real_worktree_at_mountpoint_and_umount_removes_it() {
    let td = TempDir::new().unwrap();
    let home = td.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let bare = init_bare_with_commit(td.path());
    let mountpoint = td.path().join("workspace");
    let config_path = td.path().join("ws.json");

    let config = serde_json::json!({
        "name": "git-only",
        "mountpoint": mountpoint,
        "sources": [
            {
                "path": "",
                "mount_as": "remote-lib",
                "remote_git": {
                    "url": bare,
                    "branch": null
                }
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

    let worktree = td.path().join("workspace/remote-lib");
    assert!(worktree.is_dir());
    assert!(worktree.join(".git").is_file());
    assert_eq!(
        fs::read_to_string(worktree.join("lib.rs")).unwrap(),
        "pub fn hello() {}\n"
    );

    let output = vwsd(&home)
        .args([
            "umount",
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
    assert!(!worktree.exists());
    assert!(td.path().join("workspace").exists());
}
