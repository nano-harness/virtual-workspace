//! Regression test ensuring `vwsd` still accepts workspace config files
//! written in the pre-refactor (NFS-era) format, which included fields
//! (`mode`, `upperdir`, `nfs_port`, `lock_mode`, `filters`, and per-source
//! `priority`/`access`) that no longer exist in `vwsd::model::Workspace`
//! / `Source`. Since those structs don't use `#[serde(deny_unknown_fields)]`,
//! old config files must continue to deserialize and mount successfully,
//! silently ignoring the obsolete fields.

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

/// A workspace config written exactly in the legacy (pre-refactor) shape
/// documented by the old README, including now-obsolete fields.
fn legacy_config_json(mountpoint: &Path, local: &Path, bare: &Path) -> serde_json::Value {
    serde_json::json!({
        "name": "mixed-workspace",
        "mountpoint": mountpoint,
        "mode": "cow",
        "upperdir": "/tmp/should-be-ignored/upper/mixed-workspace",
        "nfs_port": 0,
        "lock_mode": "nlm",
        "filters": {
            "include": [],
            "exclude": ["**/.git/**", "**/node_modules/**", "**/target/**"]
        },
        "sources": [
            {
                "path": local,
                "priority": 100,
                "access": "rw",
                "mount_as": "local-code"
            },
            {
                "path": "",
                "priority": 90,
                "access": "rw",
                "mount_as": "shared-lib",
                "remote_git": {
                    "url": bare,
                    "branch": "main"
                }
            }
        ]
    })
}

#[test]
fn legacy_format_config_file_still_mounts_and_round_trips() {
    let td = TempDir::new().unwrap();
    let home = td.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let bare = init_bare_with_commit(td.path());

    let local = td.path().join("local-code");
    fs::create_dir_all(&local).unwrap();
    run_git(&["init"], &local);
    run_git(&["config", "user.email", "test@test.com"], &local);
    run_git(&["config", "user.name", "Test"], &local);
    fs::write(local.join("README.md"), "hello\n").unwrap();
    run_git(&["add", "-A"], &local);
    run_git(&["commit", "-m", "init"], &local);

    let mountpoint = td.path().join("VirtualWorkspaces/mixed-workspace");
    let config_dir = home.join(".vws/workspaces");
    fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("mixed-workspace.json");
    fs::write(
        &config_path,
        serde_json::to_string_pretty(&legacy_config_json(&mountpoint, &local, &bare)).unwrap(),
    )
    .unwrap();

    // `vwsd mount` must accept the legacy config file (unknown fields are
    // ignored by serde) and produce a real, non-symlink worktree for the
    // remote source directly under the mountpoint.
    let output = vwsd(&home)
        .args(["mount", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mount failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(local.join("README.md").exists(), "local source untouched");
    let remote_worktree = mountpoint.join("shared-lib");
    assert!(
        remote_worktree.join(".git").is_file(),
        "remote worktree should be a real worktree directory, not present"
    );
    assert!(
        !remote_worktree.is_symlink(),
        "remote worktree directory must not be a symlink"
    );

    // The rewritten config should have gained a persistent `id` and no
    // longer needs to (and does not) carry the obsolete NFS-era fields.
    let rewritten: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert!(rewritten["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(rewritten.get("upperdir").is_none());
    assert!(rewritten.get("nfs_port").is_none());
    assert!(rewritten.get("lock_mode").is_none());
    assert!(rewritten.get("filters").is_none());
    assert!(rewritten.get("mode").is_none());

    // `list`/`status`, `doctor`, `umount`, and `destroy` must all keep
    // working against a workspace that originated from a legacy config.
    let output = vwsd(&home).args(["list", "--json"]).output().unwrap();
    assert!(output.status.success());
    let list: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(list["workspaces"][0]["name"], "mixed-workspace");

    let output = vwsd(&home)
        .args([
            "doctor",
            "--config",
            config_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let doctor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doctor["summary"]["failures"], 0);

    let output = vwsd(&home)
        .args(["umount", "--name", "mixed-workspace", "--force"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "umount failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!remote_worktree.exists());
    assert!(local.exists(), "local source must survive umount");

    let output = vwsd(&home)
        .args(["destroy", "--name", "mixed-workspace", "--force"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "destroy failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!config_path.exists());
    assert!(local.exists(), "local source must survive destroy");
}
