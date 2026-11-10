use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn run_git(args: &[&str], cwd: &Path) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

struct RemoteWorkspace {
    _temp: TempDir,
    home: PathBuf,
    config: PathBuf,
    mountpoint: PathBuf,
    worktree: PathBuf,
    cache: PathBuf,
}

impl RemoteWorkspace {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let home = root.join("home");
        let config_dir = home.join(".vws/workspaces");
        fs::create_dir_all(&config_dir).unwrap();

        let origin = root.join("origin.git");
        let seed = root.join("seed");
        run_git(&["init", "--bare", origin.to_str().unwrap()], root);
        run_git(
            &["clone", origin.to_str().unwrap(), seed.to_str().unwrap()],
            root,
        );
        run_git(&["config", "user.email", "test@test.com"], &seed);
        run_git(&["config", "user.name", "Test"], &seed);
        fs::write(seed.join("remote.txt"), "remote\n").unwrap();
        run_git(&["add", "-A"], &seed);
        run_git(&["commit", "-m", "init"], &seed);
        run_git(&["push"], &seed);

        let config = config_dir.join("safety.json");
        let mountpoint = root.join("workspace");
        fs::write(
            &config,
            serde_json::to_string_pretty(&serde_json::json!({
                "id": "teardown-safety",
                "name": "safety",
                "mountpoint": mountpoint,
                "sources": [{
                    "path": "",
                    "mount_as": "remote-lib",
                    "remote_git": { "url": origin, "branch": null }
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let worktree = mountpoint.join("remote-lib");
        let output = Command::new(env!("CARGO_BIN_EXE_vwsd"))
            .env("HOME", &home)
            .args(["mount", "--config"])
            .arg(&config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "mount failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(worktree.join(".git").is_file());
        let cache = worktree.join(run_git(&["rev-parse", "--git-common-dir"], &worktree));
        Self {
            _temp: temp,
            home,
            config,
            mountpoint,
            worktree,
            cache,
        }
    }

    fn teardown(&self, command: &str, force: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vwsd"));
        cmd.env("HOME", &self.home)
            .args([command, "--config"])
            .arg(&self.config);
        if force {
            cmd.arg("--force");
        }
        cmd.output().unwrap()
    }

    fn assert_teardown_refused(&self, command: &str, error: &str) {
        let original_config = fs::read(&self.config).unwrap();
        fs::write(self.worktree.join("important.txt"), "uncommitted work\n").unwrap();
        // Neither --force nor normal teardown may treat an unknown status as
        // safe. Without --force, the error must precede any interactive prompt.
        for force in [true, false] {
            let output = self.teardown(command, force);
            assert!(
                !output.status.success(),
                "{command} unexpectedly succeeded (force={force})"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(error), "{stderr}");
            assert!(stderr.contains("remote-lib"), "{stderr}");
            assert!(stderr.contains("refusing to"), "{stderr}");
            assert!(self.mountpoint.is_dir());
            assert!(self.worktree.join(".git").is_file());
            assert!(self.cache.is_dir());
            assert_eq!(fs::read(&self.config).unwrap(), original_config);
            assert_eq!(
                fs::read_to_string(self.worktree.join("important.txt")).unwrap(),
                "uncommitted work\n"
            );
        }
    }

    fn corrupt_index(&self) {
        let index = self.worktree.join(run_git(
            &["rev-parse", "--git-path", "index"],
            &self.worktree,
        ));
        fs::write(index, b"corrupt index").unwrap();
        // Branch lookup still works, so this specifically reaches the sync
        // check failure rather than the earlier branch lookup failure.
        assert!(!run_git(&["branch", "--show-current"], &self.worktree).is_empty());
    }

    fn break_git_directory(&self) {
        fs::write(
            self.worktree.join(".git"),
            "gitdir: nonexistent-git-directory\n",
        )
        .unwrap();
    }
}

#[test]
fn destroy_refuses_sync_check_errors_even_with_force() {
    let ws = RemoteWorkspace::new();
    ws.corrupt_index();
    ws.assert_teardown_refused("destroy", "cannot determine the sync status");
}

#[test]
fn umount_refuses_sync_check_errors_even_with_force() {
    let ws = RemoteWorkspace::new();
    ws.corrupt_index();
    ws.assert_teardown_refused("umount", "cannot determine the sync status");
}

#[test]
fn destroy_refuses_branch_lookup_errors_even_with_force() {
    let ws = RemoteWorkspace::new();
    ws.break_git_directory();
    ws.assert_teardown_refused("destroy", "cannot determine the current branch");
}

#[test]
fn umount_refuses_branch_lookup_errors_even_with_force() {
    let ws = RemoteWorkspace::new();
    ws.break_git_directory();
    ws.assert_teardown_refused("umount", "cannot determine the current branch");
}

#[test]
fn teardown_checks_every_source_before_removing_any_worktree() {
    let ws = RemoteWorkspace::new();
    let second = ws.mountpoint.join("second-lib");
    run_git(
        &["worktree", "add", "--detach", second.to_str().unwrap()],
        &ws.cache,
    );
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&ws.config).unwrap()).unwrap();
    let mut source = config["sources"][0].clone();
    source["mount_as"] = "second-lib".into();
    config["sources"].as_array_mut().unwrap().push(source);
    let original_config = serde_json::to_vec_pretty(&config).unwrap();
    fs::write(&ws.config, &original_config).unwrap();

    let index = second.join(run_git(&["rev-parse", "--git-path", "index"], &second));
    fs::write(index, b"corrupt index").unwrap();
    fs::write(second.join("important.txt"), "uncommitted work\n").unwrap();

    for command in ["destroy", "umount"] {
        let output = ws.teardown(command, true);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{command} unexpectedly succeeded");
        assert!(
            stderr.contains("cannot determine the sync status"),
            "{stderr}"
        );
        assert!(stderr.contains("second-lib"), "{stderr}");
        assert!(ws.worktree.join("remote.txt").is_file());
        assert!(second.join("remote.txt").is_file());
        assert!(ws.cache.is_dir());
        assert_eq!(fs::read(&ws.config).unwrap(), original_config);
        assert_eq!(
            fs::read_to_string(second.join("important.txt")).unwrap(),
            "uncommitted work\n"
        );
    }
}

#[test]
fn teardown_checks_detached_head_changes_and_commits() {
    for command in ["destroy", "umount"] {
        let ws = RemoteWorkspace::new();
        run_git(&["checkout", "--detach"], &ws.worktree);
        run_git(&["config", "user.email", "test@test.com"], &ws.worktree);
        run_git(&["config", "user.name", "Test"], &ws.worktree);
        run_git(
            &["commit", "--allow-empty", "-m", "local commit"],
            &ws.worktree,
        );
        fs::write(ws.worktree.join("important.txt"), "uncommitted work\n").unwrap();
        assert!(run_git(&["branch", "--show-current"], &ws.worktree).is_empty());

        let output = ws.teardown(command, true);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(stderr.contains("1 uncommitted change(s)"), "{stderr}");
        assert!(stderr.contains("1 unpushed commit(s)"), "{stderr}");
        assert!(!ws.worktree.exists());
        assert_eq!(ws.config.exists(), command == "umount");
    }
}
