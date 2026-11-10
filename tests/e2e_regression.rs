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
    let vscode = fs::read_to_string(&vscode_path).unwrap();
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
    // Generated editor files are user-visible files, not disposable worktrees.
    assert_eq!(fs::read_to_string(vscode_path).unwrap(), vscode);
    assert!(!config_path.exists());
}

struct CleanupFixture {
    _root: TempDir,
    home: PathBuf,
    config: PathBuf,
    mountpoint: PathBuf,
    worktree: PathBuf,
}

impl CleanupFixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let home = root.path().join("home");
        let config_dir = home.join(".vws/workspaces");
        fs::create_dir_all(&config_dir).unwrap();
        let bare = init_bare_with_commit(root.path());
        let mountpoint = root.path().join("workspace");
        let config = config_dir.join("cleanup.json");
        fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "name": "cleanup",
                "mountpoint": mountpoint,
                "sources": [{
                    "path": "",
                    "mount_as": "repo",
                    "remote_git": { "url": bare }
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let fixture = Self {
            worktree: mountpoint.join("repo"),
            _root: root,
            home,
            config,
            mountpoint,
        };
        fixture.run("mount", false);
        fixture
    }

    fn run(&self, command: &str, expect_failure: bool) {
        let mut cmd = vwsd(&self.home);
        cmd.args([command, "--config", self.config.to_str().unwrap()]);
        if command != "mount" {
            cmd.arg("--force");
        }
        let output = cmd.output().unwrap();
        assert_eq!(
            output.status.success(),
            !expect_failure,
            "{command}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn edit_config(&self, edit: impl FnOnce(&mut serde_json::Value)) {
        let mut config = serde_json::from_slice(&fs::read(&self.config).unwrap()).unwrap();
        edit(&mut config);
        fs::write(&self.config, serde_json::to_vec(&config).unwrap()).unwrap();
    }
}

#[test]
fn destroy_preserves_unrelated_mountpoint_files() {
    let f = CleanupFixture::new();
    let notes = f.mountpoint.join("notes.txt");
    fs::write(&notes, "user notes").unwrap();
    f.run("destroy", false);
    assert_eq!(fs::read_to_string(notes).unwrap(), "user notes");
    assert!(!f.worktree.exists());
    assert!(!f.config.exists());
}

#[test]
fn destroy_removes_empty_mountpoint() {
    let f = CleanupFixture::new();
    f.run("destroy", false);
    assert!(!f.mountpoint.exists());
    assert!(!f.config.exists());
}

#[test]
fn cleanup_rejects_foreign_worktree_before_removing_managed_worktree() {
    let f = CleanupFixture::new();
    let foreign = f.mountpoint.join("foreign");
    let seed = f._root.path().join("seed");
    run_git(
        &[
            "worktree",
            "add",
            "-b",
            "foreign",
            foreign.to_str().unwrap(),
        ],
        &seed,
    );
    fs::write(foreign.join("private.txt"), "keep").unwrap();
    f.edit_config(|config| {
        let mut source = config["sources"][0].clone();
        source["mount_as"] = "foreign".into();
        config["sources"].as_array_mut().unwrap().push(source);
    });
    f.run("destroy", true);
    assert!(f.worktree.join("remote.txt").exists());
    assert_eq!(
        fs::read_to_string(foreign.join("private.txt")).unwrap(),
        "keep"
    );
    assert!(f.config.exists());
}

#[test]
fn cleanup_rejects_copied_git_pointer_with_wrong_backlink() {
    let f = CleanupFixture::new();
    let fake = f.mountpoint.join("fake");
    fs::create_dir(&fake).unwrap();
    fs::copy(f.worktree.join(".git"), fake.join(".git")).unwrap();
    fs::write(fake.join("private.txt"), "keep").unwrap();
    f.edit_config(|config| config["sources"][0]["mount_as"] = "fake".into());
    f.run("umount", true);
    assert_eq!(
        fs::read_to_string(fake.join("private.txt")).unwrap(),
        "keep"
    );
    assert!(f.worktree.join("remote.txt").exists());
}

#[test]
fn cleanup_does_not_delete_after_git_refuses_locked_worktree() {
    let f = CleanupFixture::new();
    run_git(
        &["worktree", "lock", f.worktree.to_str().unwrap()],
        &f.worktree,
    );
    fs::write(f.worktree.join("private.txt"), "keep").unwrap();
    f.run("destroy", true);
    assert_eq!(
        fs::read_to_string(f.worktree.join("private.txt")).unwrap(),
        "keep"
    );
    assert!(f.config.exists());
}

#[test]
fn cleanup_preserves_local_source_inside_remote_worktree() {
    let f = CleanupFixture::new();
    let local = f.worktree.join("local-project");
    fs::create_dir(&local).unwrap();
    fs::write(local.join("notes.txt"), "keep").unwrap();
    f.edit_config(|config| {
        config["sources"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"path": local}));
    });
    f.run("destroy", true);
    assert_eq!(fs::read_to_string(local.join("notes.txt")).unwrap(), "keep");
    assert!(f.config.exists());
}

#[cfg(unix)]
#[test]
fn cleanup_rejects_symlink_in_remote_target_path() {
    let f = CleanupFixture::new();
    let outside = f._root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let moved = outside.join("repo");
    run_git(
        &[
            "worktree",
            "move",
            f.worktree.to_str().unwrap(),
            moved.to_str().unwrap(),
        ],
        &f.worktree,
    );
    std::os::unix::fs::symlink(&outside, f.mountpoint.join("group")).unwrap();
    f.edit_config(|config| config["sources"][0]["mount_as"] = "group/repo".into());
    f.run("destroy", true);
    assert!(moved.join("remote.txt").exists());
    assert!(f.config.exists());
}

#[test]
fn cleanup_mountpoint_rejects_root_and_home_aliases() {
    assert!(vwsd::mount_helper::cleanup_mountpoint(Path::new("")).is_err());
    let root_alias = std::env::temp_dir().join("..");
    if fs::canonicalize(&root_alias).unwrap() == Path::new("/") {
        assert!(vwsd::mount_helper::cleanup_mountpoint(&root_alias).is_err());
    }
    let home = dirs::home_dir().unwrap().join(".");
    assert!(vwsd::mount_helper::cleanup_mountpoint(&home).is_err());
}
