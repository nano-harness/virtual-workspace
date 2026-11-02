use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

fn ensure_git_env() {
    static GIT_ENV_ONCE: std::sync::Once = std::sync::Once::new();
    GIT_ENV_ONCE.call_once(|| {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "safe.bareRepository");
        std::env::set_var("GIT_CONFIG_VALUE_0", "all");
    });
}

pub fn run_git(args: &[&str], cwd: &Path) -> Result<String> {
    ensure_git_env();
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .context("failed to execute git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a git command with data piped to stdin (e.g. `update-ref --stdin`).
fn run_git_stdin(args: &[&str], cwd: &Path, stdin_data: &str) -> Result<String> {
    ensure_git_env();
    let mut child = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to execute git")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(stdin_data.as_bytes())
            .context("failed to write to git stdin")?;
    }
    let output = child.wait_with_output().context("failed to wait for git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Clone a repository to the given local path.
/// If `branch` is specified, only that branch is cloned.
pub fn clone(url: &str, dest: &Path, branch: Option<&str>) -> Result<()> {
    ensure_git_env();
    let dest_str = dest.to_string_lossy();
    let mut args = vec!["clone"];
    if let Some(b) = branch {
        args.extend_from_slice(&["--branch", b, "--single-branch"]);
    }
    // Use `--` to separate options from the repository URL and destination path,
    // preventing URLs that start with `-` from being interpreted as flags.
    args.extend_from_slice(&["--", url, &dest_str]);

    let output = Command::new("git")
        .args(&args)
        .output()
        .context("failed to execute git clone")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone failed: {}", stderr.trim());
    }
    Ok(())
}

/// Stage all changes in the repository.
pub fn add_all(repo: &Path) -> Result<()> {
    run_git(&["add", "-A"], repo)?;
    Ok(())
}

/// Commit staged changes with the given message.
pub fn commit(repo: &Path, message: &str) -> Result<()> {
    run_git(&["commit", "-m", message], repo)?;
    Ok(())
}

/// Push committed changes to the remote.
pub fn push(repo: &Path) -> Result<()> {
    run_git(&["push"], repo)?;
    Ok(())
}

/// Check whether the working tree has uncommitted changes.
pub fn has_changes(repo: &Path) -> Result<bool> {
    let status = run_git(&["status", "--porcelain"], repo)?;
    Ok(!status.is_empty())
}

/// Count the number of files with uncommitted changes.
pub fn count_uncommitted_changes(repo: &Path) -> Result<usize> {
    let status = run_git(&["status", "--porcelain"], repo)?;
    Ok(status.lines().filter(|l| !l.is_empty()).count())
}

#[cfg(test)]
mod count_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_repo(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .arg(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn count_uncommitted_changes_counts_correctly() {
        let td = TempDir::new().unwrap();
        let repo = td.path().join("r");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        // Start clean
        assert_eq!(count_uncommitted_changes(&repo).unwrap(), 0);

        // Add a tracked file and commit
        fs::write(repo.join("a.txt"), "a").unwrap();
        add_all(&repo).unwrap();
        commit(&repo, "init").unwrap();
        assert_eq!(count_uncommitted_changes(&repo).unwrap(), 0);

        // Modify tracked file (unstaged)
        fs::write(repo.join("a.txt"), "changed").unwrap();
        assert_eq!(count_uncommitted_changes(&repo).unwrap(), 1);

        // Add a staged new file
        fs::write(repo.join("b.txt"), "b").unwrap();
        add_all(&repo).unwrap();
        // Now: modified a.txt (staged) + staged b.txt = 2
        assert_eq!(count_uncommitted_changes(&repo).unwrap(), 2);

        // Add an untracked file (not staged)
        fs::write(repo.join("c.txt"), "c").unwrap();
        // 3 entries: modified a.txt (staged), staged b.txt, untracked c.txt
        assert_eq!(count_uncommitted_changes(&repo).unwrap(), 3);
    }
}

/// Return the current branch name.
pub fn current_branch(repo: &Path) -> Result<String> {
    run_git(&["branch", "--show-current"], repo)
}

pub fn has_head_commit(repo: &Path) -> bool {
    ref_exists(repo, "HEAD").unwrap_or(false)
}

/// Clone a repository as a bare clone (no working tree).
///
/// After cloning, the fetch refspec is configured so that `git fetch`
/// properly updates local branches from the remote.
pub fn clone_bare(url: &str, dest: &Path) -> Result<()> {
    ensure_git_env();
    let dest_str = dest.to_string_lossy();
    // Use `--` to separate options from the repository URL and destination path,
    // preventing URLs that start with `-` from being interpreted as flags.
    let output = Command::new("git")
        .args([
            "clone",
            "--bare",
            "--filter=blob:none",
            "--",
            url,
            &dest_str,
        ])
        .output()
        .context("failed to execute git clone --bare")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone --bare failed: {}", stderr.trim());
    }
    // `git clone --bare` does not create a fetch refspec, so we add one.
    // Use refs/remotes/origin/* as the destination so that `git fetch`
    // never writes into refs/heads/*, which would conflict with
    // worktree-locked branches.
    run_git(
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
        dest,
    )?;
    Ok(())
}

/// Clone a bare repository from a local shared object-pool repo instead of
/// the remote URL.  This is purely local: no network round-trip is needed
/// because refs and objects come from `shared` (objects are shared via
/// `--reference` alternates, exactly like the old network clone did).
///
/// All repository config is written via `-c` flags on the clone itself, so
/// the whole setup costs one extra git process (for `remote set-url`) on top
/// of the clone.  The promisor config makes missing blobs lazily fetchable
/// from the real remote (local transports ignore `--filter`, so the config
/// is supplied explicitly rather than relying on clone to propagate it).
/// The fetch refspec takes effect before the initial fetch, so
/// refs/remotes/origin/* are populated by the clone as well.
pub fn clone_bare_from_shared(shared: &Path, dest: &Path, remote_url: &str) -> Result<()> {
    ensure_git_env();
    let shared_str = shared.to_string_lossy();
    let dest_str = dest.to_string_lossy();
    let output = Command::new("git")
        .args([
            "clone",
            "--bare",
            "--filter=blob:none",
            "-c",
            "remote.origin.promisor=true",
            "-c",
            "remote.origin.partialclonefilter=blob:none",
            "-c",
            "remote.origin.fetch=+refs/heads/*:refs/remotes/origin/*",
            "--reference",
            &shared_str,
            "--",
            &shared_str,
            &dest_str,
        ])
        .output()
        .context("failed to execute git clone --bare from shared")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone --bare from shared failed: {}", stderr.trim());
    }
    // Point origin at the real remote (the clone recorded the local path;
    // a `-c remote.origin.url=...` flag would be overwritten by the clone).
    run_git(&["remote", "set-url", "origin", remote_url], dest)?;
    Ok(())
}

/// Fix legacy fetch refspec in existing bare repos.
///
/// Old bare repos used `+refs/heads/*:refs/heads/*` which conflicts with
/// worktree-locked branches. This migrates to `+refs/heads/*:refs/remotes/origin/*`.
/// Returns `true` if the refspec was changed, `false` if it was already correct.
pub fn fix_fetch_refspec(repo: &Path) -> Result<bool> {
    let current = run_git(&["config", "remote.origin.fetch"], repo)?;
    if current == "+refs/heads/*:refs/heads/*" {
        run_git(
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
            repo,
        )?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Migrate existing refs/heads/* to refs/remotes/origin/* in a bare repo.
///
/// After changing the fetch refspec, remote tracking refs must exist so that
/// `pull_rebase` and branch creation can reference them.
///
/// Both ref namespaces are snapshot in a single `for-each-ref` run and all
/// missing tracking refs are created with one batched `update-ref --stdin`,
/// so the cost is 2-3 git processes regardless of branch count (previously
/// 2 processes per branch).
pub fn migrate_refs_to_remotes(repo: &Path) -> Result<()> {
    let heads = run_git(
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/",
        ],
        repo,
    )?;
    if heads.is_empty() {
        return Ok(());
    }
    let remotes = run_git(
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/remotes/origin/",
        ],
        repo,
    )?;
    let existing: std::collections::HashSet<&str> = remotes.lines().collect();
    let mut batch = String::new();
    for line in heads.lines() {
        let (refname, sha) = match line.split_once(' ') {
            Some(pair) => pair,
            None => continue,
        };
        let branch = refname.strip_prefix("refs/heads/").unwrap_or(refname);
        let remote_ref = format!("refs/remotes/origin/{}", branch);
        if !existing.contains(remote_ref.as_str()) {
            batch.push_str(&format!("update {} {}\n", remote_ref, sha));
        }
    }
    if !batch.is_empty() {
        run_git_stdin(&["update-ref", "--stdin"], repo, &batch)?;
    }
    Ok(())
}

/// Check whether an arbitrary ref exists in a repository.
pub fn ref_exists(repo: &Path, refspec: &str) -> Result<bool> {
    ensure_git_env();
    let output = Command::new("git")
        .args(["rev-parse", "--verify", refspec])
        .current_dir(repo)
        .output()
        .with_context(|| format!("failed to check if ref '{}' exists", refspec))?;
    Ok(output.status.success())
}

/// Fetch all remote refs in a repository (typically a bare cache).
pub fn fetch_all(repo: &Path) -> Result<()> {
    run_git(&["fetch", "--all"], repo)?;
    Ok(())
}

/// Synchronize remote-tracking refs from a local shared bare repo into
/// another repo, replacing a network fetch with a local one.
///
/// `repo` is expected to reference `source`'s object store via alternates
/// (see `clone_bare_from_shared`): the local transport then negotiates an
/// empty pack (every object is already visible), so only ref updates cross
/// the wire-free pipe.  Semantics deliberately match `git fetch --all`
/// without `--prune`:
/// - refs/remotes/origin/* are force-updated to the source's values;
/// - refs that only exist locally are left alone (no deletion);
/// - tags pointing into the synced history are auto-followed, existing tags
///   are never clobbered.
pub fn sync_refs_from(repo: &Path, source: &Path) -> Result<()> {
    let src = source.to_string_lossy();
    run_git(
        &[
            "fetch",
            &src,
            "+refs/remotes/origin/*:refs/remotes/origin/*",
        ],
        repo,
    )?;
    Ok(())
}

/// Add a worktree at the given branch reference.
///
/// The reference is validated to prevent option injection (refs starting
/// with `-` are rejected).
pub fn worktree_add(repo: &Path, worktree_path: &Path, reference: &str) -> Result<()> {
    if reference.starts_with('-') {
        bail!("invalid git reference: {:?}", reference);
    }
    let wt_str = worktree_path.to_string_lossy();
    run_git(&["worktree", "add", &wt_str, reference], repo)?;
    Ok(())
}

/// Prune stale worktree registrations (e.g. worktree directories that were
/// deleted on disk without `git worktree remove`).
pub fn worktree_prune(repo: &Path) -> Result<()> {
    run_git(&["worktree", "prune"], repo)?;
    Ok(())
}

/// Checkout a branch in a worktree (non-detached).
pub fn worktree_checkout_branch(repo: &Path, branch: &str) -> Result<()> {
    if branch.starts_with('-') {
        bail!("invalid git branch: {:?}", branch);
    }
    run_git(&["checkout", branch], repo)?;
    Ok(())
}

/// Pull with rebase. Fetches from the remote and rebases local commits on top
/// of upstream changes.
///
/// Uses a two-step fetch + rebase to work correctly with bare-repo
/// worktrees where the fetch refspec maps directly to local branches.
///
/// If the caller has already called `fetch_all` on the bare cache, use
/// `rebase_onto_tracking` instead to avoid a redundant network round-trip.
pub fn pull_rebase(repo: &Path, branch: &str) -> Result<()> {
    if branch.starts_with('-') {
        bail!("invalid git branch: {:?}", branch);
    }
    let remote_ref = format!("refs/remotes/origin/{}", branch);
    let refspec = format!("+refs/heads/{}:{}", branch, remote_ref);
    // Use --refmap= to prevent the bare repo's configured refspec from
    // also updating refs/heads/<branch> (which is checked out and locked).
    run_git(&["fetch", "--refmap=", "origin", &refspec], repo)?;
    run_git(&["rebase", &remote_ref], repo)?;
    Ok(())
}

/// Rebase local commits onto the already-fetched remote-tracking branch.
///
/// Unlike `pull_rebase`, this does **not** fetch from the remote first.
/// Use this when the caller has already run `fetch_all` on the shared bare
/// cache, so the `refs/remotes/origin/<branch>` tracking ref is up to date
/// and a second network round-trip would be redundant.
pub fn rebase_onto_tracking(repo: &Path, branch: &str) -> Result<()> {
    if branch.starts_with('-') {
        bail!("invalid git branch: {:?}", branch);
    }
    let remote_ref = format!("refs/remotes/origin/{}", branch);
    run_git(&["rebase", &remote_ref], repo)?;
    Ok(())
}

/// Abort an in-progress rebase, restoring the worktree to its pre-rebase state.
///
/// This is a best-effort operation: if the abort itself fails (e.g. no rebase
/// in progress), the error is silently ignored by callers.
pub fn rebase_abort(repo: &Path) -> Result<()> {
    run_git(&["rebase", "--abort"], repo)?;
    Ok(())
}

/// Check whether there are local commits that have not been pushed to the
/// remote tracking branch (`origin/<branch>`).
///
/// Returns `true` when unpushed commits exist or when the remote tracking ref
/// does not yet exist (i.e. the branch has never been pushed).
pub fn has_unpushed_commits(repo: &Path, branch: &str) -> Result<bool> {
    let remote_ref = format!("refs/remotes/origin/{}", branch);
    if !ref_exists(repo, &remote_ref)? {
        // Remote ref absent (new branch) — treat as having unpushed commits.
        return Ok(true);
    }
    let output = run_git(
        &["rev-list", &format!("{}..HEAD", remote_ref), "--count"],
        repo,
    )?;
    let count: usize = output.trim().parse().unwrap_or(0);
    Ok(count > 0)
}

/// Return the number of local commits that have not been pushed to the remote
/// tracking branch (`origin/<branch>`).
///
/// When the remote tracking ref does not exist (branch never pushed), counts
/// commits reachable from HEAD that are not reachable from any `origin/*` ref,
/// avoiding inflating the count with the entire history shared with other branches.
pub fn count_unpushed_commits(repo: &Path, branch: &str) -> Result<usize> {
    if !has_head_commit(repo) {
        return Ok(0);
    }
    let remote_ref = format!("refs/remotes/origin/{}", branch);
    if !ref_exists(repo, &remote_ref)? {
        // Branch never pushed: count commits not reachable from any remote ref.
        // NOTE: HEAD must appear before --not so it is treated as a positive ref;
        // arguments after --not are all treated as exclusions.
        let output = run_git(
            &["rev-list", "--count", "HEAD", "--not", "--remotes=origin"],
            repo,
        )?;
        return Ok(output.trim().parse().unwrap_or(0));
    }
    let output = run_git(
        &["rev-list", &format!("{}..HEAD", remote_ref), "--count"],
        repo,
    )?;
    Ok(output.trim().parse().unwrap_or(0))
}

/// Return the default branch name by reading HEAD.
///
/// In a bare repository cloned with `git clone --bare`, HEAD points to the
/// default branch (e.g. `refs/heads/main`).
pub fn default_branch(repo: &Path) -> Result<String> {
    let head = run_git(&["symbolic-ref", "HEAD"], repo)?;
    Ok(head
        .strip_prefix("refs/heads/")
        .unwrap_or(&head)
        .to_string())
}

/// List branch names under `refs/remotes/origin/` (excluding the HEAD symref).
pub fn remote_branches(repo: &Path) -> Result<Vec<String>> {
    let out = run_git(
        &[
            "for-each-ref",
            "--format=%(refname:strip=3)",
            "refs/remotes/origin",
        ],
        repo,
    )?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "HEAD")
        .map(str::to_string)
        .collect())
}

/// Check whether a branch exists in a (bare) repository.
pub fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    ensure_git_env();
    let refspec = format!("refs/heads/{}", branch);
    let output = Command::new("git")
        .args(["rev-parse", "--verify", &refspec])
        .current_dir(repo)
        .output()
        .with_context(|| format!("failed to check if branch '{}' exists", branch))?;
    Ok(output.status.success())
}

/// Create a new branch in a (bare) repository based on an existing start point.
///
/// Both the branch name and the start point are validated to prevent option
/// injection (values starting with `-` are rejected).
pub fn create_branch(repo: &Path, new_branch: &str, start_point: &str) -> Result<()> {
    if new_branch.starts_with('-') {
        bail!("invalid git branch: {:?}", new_branch);
    }
    if start_point.starts_with('-') {
        bail!("invalid git reference: {:?}", start_point);
    }
    run_git(&["branch", new_branch, start_point], repo).with_context(|| {
        format!(
            "failed to create branch '{}' from '{}'",
            new_branch, start_point
        )
    })?;
    Ok(())
}

/// Set the upstream tracking configuration for a branch in the bare repository.
///
/// Sets `branch.<name>.remote = <remote>` and `branch.<name>.merge = refs/heads/<name>`
/// in the bare repository's config only when the remote tracking ref
/// `refs/remotes/origin/<branch>` already exists.  When the branch is purely local
/// (no remote counterpart yet) the function is a no-op so that callers can always
/// invoke it without error.  Because the bare repo is the common-dir for all
/// worktrees derived from it, this setting is automatically inherited by every
/// linked worktree — `git status` in any worktree will then show the correct
/// upstream information.
///
/// `git config` is used directly rather than `git branch --set-upstream-to` because
/// the latter may behave inconsistently when called from inside a bare repo worktree.
pub fn set_upstream(repo: &Path, branch: &str, remote: &str) -> Result<()> {
    let remote_ref = format!("refs/remotes/{}/{}", remote, branch);
    if !ref_exists(repo, &remote_ref)? {
        return Ok(());
    }
    let remote_key = format!("branch.{}.remote", branch);
    let merge_key = format!("branch.{}.merge", branch);
    let merge_val = format!("refs/heads/{}", branch);
    // Read all branch config in one process and only write what is actually
    // missing/wrong.  resolve() calls this on every mount, so the steady
    // state is "already correct": 2 git processes (ref_exists + get-regexp)
    // instead of 4.  `--get-regexp` exits 1 when nothing matches, hence the
    // unwrap_or_default.
    let current = run_git(&["config", "--get-regexp", "^branch\\."], repo).unwrap_or_default();
    let want_remote = format!("{} {}", remote_key, remote);
    let want_merge = format!("{} {}", merge_key, merge_val);
    let mut has_remote = false;
    let mut has_merge = false;
    for line in current.lines() {
        if line == want_remote {
            has_remote = true;
        } else if line == want_merge {
            has_merge = true;
        }
    }
    if !has_remote {
        run_git(&["config", &remote_key, remote], repo)
            .with_context(|| format!("failed to set upstream remote for branch '{}'", branch))?;
    }
    if !has_merge {
        run_git(&["config", &merge_key, &merge_val], repo)
            .with_context(|| format!("failed to set upstream merge for branch '{}'", branch))?;
    }
    Ok(())
}

/// Remove a worktree from a bare repository.
///
/// Uses `--force` to remove the worktree entry even when the worktree directory
/// is missing (e.g. it has already been deleted on disk).  `--` separates the
/// path from options to avoid treating paths starting with `-` as flags.
pub fn worktree_remove(bare: &Path, worktree: &Path) -> Result<()> {
    let wt_str = worktree.to_string_lossy();
    run_git(&["worktree", "remove", "--force", "--", &wt_str], bare)?;
    Ok(())
}

/// Push HEAD to a specific branch on origin.
///
/// The branch name is validated to prevent option injection (names starting
/// with `-` are rejected).
pub fn push_to_branch(repo: &Path, branch: &str) -> Result<()> {
    if branch.starts_with('-') {
        bail!("invalid git branch: {:?}", branch);
    }
    let refspec = format!("HEAD:refs/heads/{}", branch);
    run_git(&["push", "origin", &refspec], repo)?;
    Ok(())
}

/// Return the current HEAD commit hash (full SHA).
pub fn head_rev(repo: &Path) -> Result<String> {
    run_git(&["rev-parse", "HEAD"], repo)
}

/// List files changed between two commits (relative to the repo root).
///
/// Uses `git diff-tree -r --name-only` which is much faster than `git ls-tree`
/// for repositories with many files, since it only examines changed paths.
///
/// Both commit references are validated to prevent option injection (values
/// starting with `-` are rejected).
pub fn diff_tree_files(repo: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    if from.starts_with('-') || to.starts_with('-') {
        bail!("invalid git reference: {:?} {:?}", from, to);
    }
    let output = run_git(
        &["diff-tree", "-r", "--name-only", "--no-commit-id", from, to],
        repo,
    )?;
    Ok(output
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect())
}

/// Fetch remote refs for a worktree, updating the remote tracking branch.
///
/// Uses the same fetch strategy as `pull_rebase`: `--refmap=` prevents the
/// bare repo's configured refspec from also updating the checked-out branch
/// (which is locked by the worktree).
pub fn fetch_worktree(repo: &Path, branch: &str) -> Result<()> {
    if branch.starts_with('-') {
        bail!("invalid git branch: {:?}", branch);
    }
    let remote_ref = format!("refs/remotes/origin/{}", branch);
    let refspec = format!("+refs/heads/{}:{}", branch, remote_ref);
    run_git(&["fetch", "--refmap=", "origin", &refspec], repo)?;
    Ok(())
}

/// Count the number of commits the local branch is behind the remote tracking branch.
///
/// Returns 0 if the remote tracking branch does not exist (branch has never been pushed).
pub fn count_behind_commits(repo: &Path, branch: &str) -> Result<usize> {
    if !has_head_commit(repo) {
        return Ok(0);
    }
    let remote_ref = format!("refs/remotes/origin/{}", branch);
    if !ref_exists(repo, &remote_ref)? {
        return Ok(0);
    }
    let output = run_git(
        &["rev-list", &format!("HEAD..{}", remote_ref), "--count"],
        repo,
    )?;
    Ok(output.trim().parse().unwrap_or(0))
}

/// Branch synchronization status containing three metrics.
pub struct BranchSyncStatus {
    pub uncommitted_changes: usize,
    pub unpushed_commits: usize,
    pub behind_commits: usize,
}

impl BranchSyncStatus {
    /// Returns true if the branch is fully synchronized (no uncommitted changes,
    /// no unpushed commits, and not behind remote).
    pub fn is_fully_synced(&self) -> bool {
        self.uncommitted_changes == 0 && self.unpushed_commits == 0 && self.behind_commits == 0
    }
}

/// Parsed subset of `git status --porcelain=v2 --branch` output.
struct PorcelainV2Status {
    /// Number of change entries: tracked modifications (`1`/`2` lines),
    /// unmerged paths (`u` lines) and untracked files (`?` lines) —
    /// everything that is not a `#` header line.  This matches the semantics
    /// of counting non-empty lines of `git status --porcelain` (v1), where
    /// untracked files appear as `??` lines and are included in the count.
    changed_entries: usize,
    /// `(ahead, behind)` relative to the configured upstream, from the
    /// `# branch.ab +A -B` header.  `None` when no upstream is configured or
    /// the upstream ref is gone (git omits the header in both cases).
    upstream_ahead_behind: Option<(usize, usize)>,
}

/// Parse the output of `git status --porcelain=v2 --branch`.
///
/// Pure function so it can be unit-tested with canned output.
fn parse_porcelain_v2_status(output: &str) -> PorcelainV2Status {
    let mut changed_entries = 0;
    let mut upstream_ahead_behind = None;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            // Format: "+<ahead> -<behind>"
            let mut ahead = None;
            let mut behind = None;
            for token in rest.split_whitespace() {
                if let Some(a) = token.strip_prefix('+') {
                    ahead = a.parse::<usize>().ok();
                } else if let Some(b) = token.strip_prefix('-') {
                    behind = b.parse::<usize>().ok();
                }
            }
            if let (Some(a), Some(b)) = (ahead, behind) {
                upstream_ahead_behind = Some((a, b));
            }
        } else if !line.is_empty() && !line.starts_with('#') {
            changed_entries += 1;
        }
    }
    PorcelainV2Status {
        changed_entries,
        upstream_ahead_behind,
    }
}

/// Fallback ahead/behind computation for [`check_branch_sync`] when
/// `git status` reports no upstream information for the checked-out branch
/// (upstream never configured, upstream ref gone, or detached HEAD).
///
/// Mirrors the historical behaviour exactly:
/// - if `refs/remotes/origin/<branch>` is missing, unpushed = commits
///   reachable from HEAD but not from any `origin/*` ref, behind = 0;
/// - otherwise unpushed = `<remote_ref>..HEAD`, behind = `HEAD..<remote_ref>`
///   (both from a single `rev-list --left-right --count` call).
fn count_ahead_behind_fallback(repo: &Path, branch: &str) -> Result<(usize, usize)> {
    if !has_head_commit(repo) {
        return Ok((0, 0));
    }
    let remote_ref = format!("refs/remotes/origin/{}", branch);
    if !ref_exists(repo, &remote_ref)? {
        // Branch never pushed: count commits not reachable from any remote ref.
        // NOTE: HEAD must appear before --not so it is treated as a positive ref;
        // arguments after --not are all treated as exclusions.
        let output = run_git(
            &["rev-list", "--count", "HEAD", "--not", "--remotes=origin"],
            repo,
        )?;
        return Ok((output.trim().parse().unwrap_or(0), 0));
    }
    // Output format: "<ahead>\t<behind>" for HEAD...<remote_ref>
    let output = run_git(
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{}", remote_ref),
        ],
        repo,
    )?;
    let mut parts = output.split_whitespace();
    let ahead = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok((ahead, behind))
}

/// Check the synchronization status of a branch against its remote tracking branch.
///
/// Uses a single `git status --porcelain=v2 --branch` invocation instead of
/// four separate git processes.  Ahead/behind counts come from the
/// `# branch.ab` header; when that header is absent (no upstream configured
/// or upstream ref gone), falls back to explicit `rev-list` counts against
/// `refs/remotes/origin/<branch>` with the same semantics as before.
///
/// Returns a `BranchSyncStatus` with counts of uncommitted changes, unpushed commits,
/// and commits behind the remote.
pub fn check_branch_sync(repo: &Path, branch: &str) -> Result<BranchSyncStatus> {
    let status = run_git(&["status", "--porcelain=v2", "--branch"], repo)?;
    let parsed = parse_porcelain_v2_status(&status);
    let (unpushed_commits, behind_commits) = match parsed.upstream_ahead_behind {
        Some((ahead, behind)) => (ahead, behind),
        None => count_ahead_behind_fallback(repo, branch)?,
    };
    Ok(BranchSyncStatus {
        uncommitted_changes: parsed.changed_entries,
        unpushed_commits,
        behind_commits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Pull the latest changes in a repository.
    ///
    /// Only used by tests; production code uses `pull_rebase` /
    /// `rebase_onto_tracking` instead.
    fn pull(repo: &Path) -> Result<()> {
        run_git(&["pull", "--ff-only"], repo)?;
        Ok(())
    }

    fn init_bare(dir: &Path) {
        Command::new("git")
            .args(["init", "--bare"])
            .arg(dir)
            .output()
            .unwrap();
    }

    fn init_repo(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .arg(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn clone_and_pull() {
        let td = TempDir::new().unwrap();
        let bare = td.path().join("bare.git");
        init_bare(&bare);

        // Create an initial commit in the bare repo via a temp clone
        let tmp_clone = td.path().join("tmp");
        clone(bare.to_str().unwrap(), &tmp_clone, None).unwrap();
        init_repo(&tmp_clone); // set user config
        fs::write(tmp_clone.join("hello.txt"), "world").unwrap();
        add_all(&tmp_clone).unwrap();
        commit(&tmp_clone, "init").unwrap();
        push(&tmp_clone).unwrap();

        // Now clone again and verify
        let work = td.path().join("work");
        clone(bare.to_str().unwrap(), &work, None).unwrap();
        assert!(work.join("hello.txt").exists());
        pull(&work).unwrap();
    }

    #[test]
    fn has_changes_detects_modifications() {
        let td = TempDir::new().unwrap();
        let repo = td.path().join("r");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        fs::write(repo.join("a.txt"), "a").unwrap();
        add_all(&repo).unwrap();
        commit(&repo, "first").unwrap();

        assert!(!has_changes(&repo).unwrap());
        fs::write(repo.join("b.txt"), "b").unwrap();
        assert!(has_changes(&repo).unwrap());
    }

    #[test]
    fn custom_commit_message() {
        let td = TempDir::new().unwrap();
        let repo = td.path().join("r");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        fs::write(repo.join("f.txt"), "data").unwrap();
        add_all(&repo).unwrap();
        commit(&repo, "vws: custom sync message").unwrap();

        let log = run_git(&["log", "--oneline", "-1"], &repo).unwrap();
        assert!(log.contains("vws: custom sync message"));
    }

    #[test]
    fn bare_clone_and_worktree() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        // Seed the remote with an initial commit
        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("file.txt"), "hello").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        // Clone as bare
        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        assert!(!cache.join("file.txt").exists()); // bare has no working tree

        // Get default branch
        let branch = default_branch(&cache).unwrap();
        assert!(!branch.is_empty());

        // Create worktree
        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        assert!(wt.join("file.txt").exists());
        assert!(wt.join(".git").is_file()); // worktree has .git file, not dir

        // Make a change in worktree and push (set user config without re-init)
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&wt)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&wt)
            .output()
            .unwrap();
        fs::write(wt.join("new.txt"), "new content").unwrap();
        add_all(&wt).unwrap();
        commit(&wt, "add new file").unwrap();
        push_to_branch(&wt, &branch).unwrap();

        // Verify push reached the original remote
        let verify = td.path().join("verify");
        clone(bare_remote.to_str().unwrap(), &verify, None).unwrap();
        assert!(verify.join("new.txt").exists());
    }

    #[test]
    fn pull_rebase_updates_worktree() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("a.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        // Clone bare and create worktree
        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        let branch = default_branch(&cache).unwrap();
        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        assert_eq!(fs::read_to_string(wt.join("a.txt")).unwrap(), "v1");

        // Push a new commit to the remote via the seed clone
        fs::write(seed.join("a.txt"), "v2").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "update").unwrap();
        push(&seed).unwrap();

        // Pull with rebase in the worktree to get remote changes
        pull_rebase(&wt, &branch).unwrap();
        assert_eq!(fs::read_to_string(wt.join("a.txt")).unwrap(), "v2");
    }

    #[test]
    fn blobless_bare_clone_works() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        // Seed the remote with a commit
        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("data.txt"), "blobless test").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        // Blobless bare clone
        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();

        // Create worktree and verify file is accessible (blob fetched on demand)
        let branch = default_branch(&cache).unwrap();
        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        assert_eq!(
            fs::read_to_string(wt.join("data.txt")).unwrap(),
            "blobless test"
        );
    }

    #[test]
    fn branch_exists_returns_true_for_existing() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "x").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        let branch = default_branch(&cache).unwrap();
        assert!(branch_exists(&cache, &branch).unwrap());
    }

    #[test]
    fn branch_exists_returns_false_for_missing() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "x").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        assert!(!branch_exists(&cache, "nonexistent-branch").unwrap());
    }

    #[test]
    fn create_branch_from_existing() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "original").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        let default = default_branch(&cache).unwrap();

        // Create a new branch from the default branch
        create_branch(&cache, "feature/new", &default).unwrap();
        assert!(branch_exists(&cache, "feature/new").unwrap());

        // Worktree from new branch should have the same content
        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, "feature/new").unwrap();
        assert_eq!(fs::read_to_string(wt.join("f.txt")).unwrap(), "original");
    }

    #[test]
    fn fix_fetch_refspec_migrates_legacy() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "hello").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        // Simulate a legacy bare clone with old refspec
        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        // Override with the legacy refspec to simulate old state
        run_git(
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/heads/*",
            ],
            &cache,
        )
        .unwrap();

        // fix_fetch_refspec should detect and migrate the legacy refspec
        let migrated = fix_fetch_refspec(&cache).unwrap();
        assert!(migrated, "should have migrated legacy refspec");

        // After migration, refspec should be updated
        let new_refspec = run_git(&["config", "remote.origin.fetch"], &cache).unwrap();
        assert_eq!(new_refspec, "+refs/heads/*:refs/remotes/origin/*");

        // Calling again should be a no-op (returns false)
        let migrated_again = fix_fetch_refspec(&cache).unwrap();
        assert!(!migrated_again, "should not migrate again");
    }

    #[test]
    fn migrate_refs_to_remotes_creates_tracking_refs() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "hello").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        // Simulate a legacy bare clone (refs/heads/* populated, refs/remotes/origin/* absent)
        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        run_git(
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/heads/*",
            ],
            &cache,
        )
        .unwrap();
        // Fetch with old refspec populates refs/heads/*
        run_git(&["fetch", "--all"], &cache).unwrap();

        let branch = default_branch(&cache).unwrap();

        // refs/remotes/origin/<branch> should not exist yet
        let remote_ref = format!("refs/remotes/origin/{}", branch);
        assert!(!ref_exists(&cache, &remote_ref).unwrap());

        // After migration, refs/remotes/origin/* should be created
        migrate_refs_to_remotes(&cache).unwrap();
        assert!(ref_exists(&cache, &remote_ref).unwrap());
    }

    #[test]
    fn ref_exists_returns_correct_results() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "data").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        // refs/heads/* is populated by `git clone --bare` regardless of the fetch refspec
        assert!(ref_exists(&cache, &format!("refs/heads/{}", branch)).unwrap());
        assert!(!ref_exists(&cache, "refs/heads/nonexistent-branch").unwrap());

        // refs/remotes/origin/* is only populated after running `git fetch`
        // (clone_bare sets the refspec but does not run an initial fetch)
        assert!(!ref_exists(&cache, &format!("refs/remotes/origin/{}", branch)).unwrap());
        fetch_all(&cache).unwrap();
        assert!(ref_exists(&cache, &format!("refs/remotes/origin/{}", branch)).unwrap());
    }

    #[test]
    fn has_unpushed_commits_no_remote_ref() {
        // When remote tracking ref is absent (branch never pushed), treat as unpushed.
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        init_repo(&wt);

        // Create a new branch in the worktree that has no remote tracking ref.
        Command::new("git")
            .args(["checkout", "-b", "new-branch"])
            .current_dir(&wt)
            .output()
            .unwrap();
        fs::write(wt.join("x.txt"), "x").unwrap();
        add_all(&wt).unwrap();
        commit(&wt, "local commit").unwrap();

        // No remote tracking ref for "new-branch" → treated as unpushed.
        assert!(has_unpushed_commits(&wt, "new-branch").unwrap());
        assert!(count_unpushed_commits(&wt, "new-branch").unwrap() >= 1);
    }

    #[test]
    fn has_unpushed_commits_after_push() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        init_repo(&wt);

        // After a local commit and push, should have no unpushed commits.
        fs::write(wt.join("new.txt"), "new").unwrap();
        add_all(&wt).unwrap();
        commit(&wt, "add new.txt").unwrap();

        // Before push: should detect unpushed commits.
        assert!(has_unpushed_commits(&wt, &branch).unwrap());
        assert_eq!(count_unpushed_commits(&wt, &branch).unwrap(), 1);

        // After push: should have no unpushed commits.
        push_to_branch(&wt, &branch).unwrap();
        // Fetch all to update tracking refs in the worktree's gitdir.
        fetch_all(&cache).unwrap();
        assert!(!has_unpushed_commits(&wt, &branch).unwrap());
        assert_eq!(count_unpushed_commits(&wt, &branch).unwrap(), 0);
    }

    #[test]
    fn worktree_remove_succeeds() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "data").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        assert!(wt.exists());

        // Remove the worktree from the bare repo.
        worktree_remove(&cache, &wt).unwrap();
    }

    #[test]
    fn test_rebase_abort_restores_state() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        // Seed the remote
        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("conflict.txt"), "base content").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        // Clone bare and create worktree
        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();
        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        init_repo(&wt);

        // Make a local commit in the worktree (conflicting change)
        fs::write(wt.join("conflict.txt"), "local version").unwrap();
        add_all(&wt).unwrap();
        commit(&wt, "local change").unwrap();

        // Push a conflicting remote commit via seed
        fs::write(seed.join("conflict.txt"), "remote version").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "remote change").unwrap();
        push(&seed).unwrap();

        // Fetch the remote change into our bare cache
        fetch_all(&cache).unwrap();

        // Rebase should fail due to conflict
        let rebase_result = rebase_onto_tracking(&wt, &branch);
        assert!(rebase_result.is_err(), "rebase should fail on conflict");

        // rebase_abort should restore the worktree to a clean state
        rebase_abort(&wt).unwrap();

        // After abort, we should not be in a rebase state
        // (the REBASE_HEAD file should not exist)
        let git_file = wt.join(".git"); // .git is a file (not dir) in worktrees
                                        // Check the actual gitdir for REBASE_HEAD
        let gitdir_content = fs::read_to_string(git_file).unwrap();
        let gitdir = gitdir_content
            .strip_prefix("gitdir: ")
            .unwrap_or("")
            .trim()
            .to_string();
        let rebase_head_file = if gitdir.starts_with('/') {
            std::path::PathBuf::from(&gitdir).join("REBASE_HEAD")
        } else {
            wt.join(&gitdir).join("REBASE_HEAD")
        };
        assert!(
            !rebase_head_file.exists(),
            "REBASE_HEAD should not exist after abort"
        );

        // Worktree should be back to local commit
        let log = run_git(&["log", "--oneline", "-1"], &wt).unwrap();
        assert!(
            log.contains("local change"),
            "local commit should be restored after abort"
        );
    }

    #[test]
    fn test_rebase_abort_noop_when_no_rebase() {
        // When no rebase is in progress, rebase --abort should return an error
        // (not panic). Callers ignore the error, so this just verifies it doesn't panic.
        let td = TempDir::new().unwrap();
        let repo = td.path().join("r");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        fs::write(repo.join("a.txt"), "hello").unwrap();
        add_all(&repo).unwrap();
        commit(&repo, "init").unwrap();

        // No rebase in progress — should return Err (not panic)
        let result = rebase_abort(&repo);
        assert!(
            result.is_err(),
            "rebase_abort should fail when no rebase is in progress"
        );
    }

    #[test]
    fn test_count_behind_commits_when_synced() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();

        // Worktree is at same commit as remote — should be 0 behind
        assert_eq!(count_behind_commits(&wt, &branch).unwrap(), 0);
    }

    #[test]
    fn test_count_behind_commits_when_behind() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();

        // Push new commits to remote via seed
        fs::write(seed.join("f.txt"), "v2").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "update 1").unwrap();
        push(&seed).unwrap();

        fs::write(seed.join("f.txt"), "v3").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "update 2").unwrap();
        push(&seed).unwrap();

        // Fetch the remote changes into worktree
        fetch_worktree(&wt, &branch).unwrap();

        // Worktree is 2 commits behind remote
        assert_eq!(count_behind_commits(&wt, &branch).unwrap(), 2);
    }

    #[test]
    fn test_check_branch_sync_fully_synced() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();

        // Worktree is fully synced
        let status = check_branch_sync(&wt, &branch).unwrap();
        assert!(status.is_fully_synced());
        assert_eq!(status.uncommitted_changes, 0);
        assert_eq!(status.unpushed_commits, 0);
        assert_eq!(status.behind_commits, 0);
    }

    #[test]
    fn test_check_branch_sync_with_changes() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        init_repo(&wt);

        // Add local commits
        fs::write(wt.join("local.txt"), "local").unwrap();
        add_all(&wt).unwrap();
        commit(&wt, "local commit").unwrap();

        // Add uncommitted changes AFTER the commit
        fs::write(wt.join("new.txt"), "new").unwrap();

        // Push remote commits via seed to make worktree behind
        fs::write(seed.join("remote.txt"), "remote").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "remote commit").unwrap();
        push(&seed).unwrap();

        // Fetch remote changes
        fetch_worktree(&wt, &branch).unwrap();

        let status = check_branch_sync(&wt, &branch).unwrap();
        assert!(!status.is_fully_synced());
        assert_eq!(status.uncommitted_changes, 1); // new.txt is untracked
        assert_eq!(status.unpushed_commits, 1); // local commit
        assert_eq!(status.behind_commits, 1); // remote commit
    }

    #[test]
    fn parse_porcelain_v2_status_with_upstream_and_changes() {
        let sample = "\
# branch.oid 4b825dc642cb6eb9a060e54bf8d69288fbee4904
# branch.head main
# branch.upstream origin/main
# branch.ab +2 -3
1 .M N... 100644 100644 100644 abc1234 abc1234 modified.txt
2 R. N... 100644 100644 100644 abc1234 abc1234 R100 new-name.txt\told-name.txt
u UU N... 100644 100644 100644 100644 abc1234 abc1234 abc1234 conflicted.txt
? untracked.txt
";
        let parsed = parse_porcelain_v2_status(sample);
        assert_eq!(parsed.changed_entries, 4);
        assert_eq!(parsed.upstream_ahead_behind, Some((2, 3)));
    }

    #[test]
    fn parse_porcelain_v2_status_no_upstream() {
        // No `# branch.upstream` / `# branch.ab` headers (branch never pushed).
        let sample = "\
# branch.oid 4b825dc642cb6eb9a060e54bf8d69288fbee4904
# branch.head feature
1 .M N... 100644 100644 100644 abc1234 abc1234 tracked.txt
";
        let parsed = parse_porcelain_v2_status(sample);
        assert_eq!(parsed.changed_entries, 1);
        assert_eq!(parsed.upstream_ahead_behind, None);
    }

    #[test]
    fn parse_porcelain_v2_status_clean_and_synced() {
        let sample = "\
# branch.oid 4b825dc642cb6eb9a060e54bf8d69288fbee4904
# branch.head main
# branch.upstream origin/main
# branch.ab +0 -0
";
        let parsed = parse_porcelain_v2_status(sample);
        assert_eq!(parsed.changed_entries, 0);
        assert_eq!(parsed.upstream_ahead_behind, Some((0, 0)));
    }

    #[test]
    fn parse_porcelain_v2_status_detached_head() {
        // Detached HEAD: no upstream/ab headers; untracked entry still counts.
        let sample = "\
# branch.oid 4b825dc642cb6eb9a060e54bf8d69288fbee4904
# branch.head (detached)
? scratch.txt
";
        let parsed = parse_porcelain_v2_status(sample);
        assert_eq!(parsed.changed_entries, 1);
        assert_eq!(parsed.upstream_ahead_behind, None);
    }

    #[test]
    fn parse_porcelain_v2_status_empty_output() {
        let parsed = parse_porcelain_v2_status("");
        assert_eq!(parsed.changed_entries, 0);
        assert_eq!(parsed.upstream_ahead_behind, None);
    }

    #[test]
    fn test_check_branch_sync_with_upstream_configured() {
        // Same scenario as test_check_branch_sync_with_changes, but with the
        // upstream tracking config set (as resolve() does via set_upstream),
        // so `git status` reports `# branch.ab` and the fast path is used
        // instead of the rev-list fallback.
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        let wt = td.path().join("wt");
        worktree_add(&cache, &wt, &branch).unwrap();
        init_repo(&wt);
        set_upstream(&cache, &branch, "origin").unwrap();

        // Sanity: the status output really carries the ahead/behind header,
        // so this test exercises the fast path (not the rev-list fallback).
        let raw = run_git(&["status", "--porcelain=v2", "--branch"], &wt).unwrap();
        assert!(
            raw.contains("# branch.ab"),
            "upstream header missing from status output: {raw}"
        );

        // Add a local commit and an uncommitted file.
        fs::write(wt.join("local.txt"), "local").unwrap();
        add_all(&wt).unwrap();
        commit(&wt, "local commit").unwrap();
        fs::write(wt.join("new.txt"), "new").unwrap();

        // Push a remote commit via seed and update the tracking ref.
        fs::write(seed.join("remote.txt"), "remote").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "remote commit").unwrap();
        push(&seed).unwrap();
        fetch_worktree(&wt, &branch).unwrap();

        let status = check_branch_sync(&wt, &branch).unwrap();
        assert!(!status.is_fully_synced());
        assert_eq!(status.uncommitted_changes, 1); // new.txt is untracked
        assert_eq!(status.unpushed_commits, 1); // local commit
        assert_eq!(status.behind_commits, 1); // remote commit
    }

    #[test]
    fn clone_bare_from_shared_creates_isolated_repo() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "shared-pool").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        // Shared object pool, cloned from the (local) remote as in resolve().
        let shared = td.path().join("shared.git");
        clone_bare(bare_remote.to_str().unwrap(), &shared).unwrap();

        // Isolated bare cloned purely locally from the shared pool.
        let isolated = td.path().join("isolated.git");
        clone_bare_from_shared(&shared, &isolated, bare_remote.to_str().unwrap()).unwrap();

        // origin points at the real remote, not the local shared path.
        let url = run_git(&["config", "remote.origin.url"], &isolated).unwrap();
        assert_eq!(url, bare_remote.to_string_lossy().to_string());

        // Fetch refspec + promisor config are in place.
        let refspec = run_git(&["config", "remote.origin.fetch"], &isolated).unwrap();
        assert_eq!(refspec, "+refs/heads/*:refs/remotes/origin/*");
        let promisor = run_git(&["config", "remote.origin.promisor"], &isolated).unwrap();
        assert_eq!(promisor, "true");
        let filter = run_git(&["config", "remote.origin.partialclonefilter"], &isolated).unwrap();
        assert_eq!(filter, "blob:none");

        // Objects are shared with the pool via alternates.
        let alternates = fs::read_to_string(isolated.join("objects/info/alternates")).unwrap();
        assert!(alternates.contains(&shared.join("objects").to_string_lossy().to_string()));

        // Refs were copied from the shared repo and a worktree can be used.
        let branch = default_branch(&isolated).unwrap();
        assert!(branch_exists(&isolated, &branch).unwrap());
        let wt = td.path().join("wt");
        worktree_add(&isolated, &wt, &branch).unwrap();
        assert_eq!(fs::read_to_string(wt.join("f.txt")).unwrap(), "shared-pool");
    }

    #[test]
    fn set_upstream_is_idempotent_and_repairs_wrong_values() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "x").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        fetch_all(&cache).unwrap();
        let branch = default_branch(&cache).unwrap();

        // First call writes the config; second call must be a no-op that
        // leaves the same values in place.
        set_upstream(&cache, &branch, "origin").unwrap();
        set_upstream(&cache, &branch, "origin").unwrap();
        let remote_cfg =
            run_git(&["config", &format!("branch.{}.remote", branch)], &cache).unwrap();
        let merge_cfg = run_git(&["config", &format!("branch.{}.merge", branch)], &cache).unwrap();
        assert_eq!(remote_cfg, "origin");
        assert_eq!(merge_cfg, format!("refs/heads/{}", branch));

        // Corrupt the values; set_upstream must repair them.
        run_git(
            &["config", &format!("branch.{}.remote", branch), "bogus"],
            &cache,
        )
        .unwrap();
        run_git(
            &[
                "config",
                &format!("branch.{}.merge", branch),
                "refs/heads/bogus",
            ],
            &cache,
        )
        .unwrap();
        set_upstream(&cache, &branch, "origin").unwrap();
        let remote_cfg =
            run_git(&["config", &format!("branch.{}.remote", branch)], &cache).unwrap();
        let merge_cfg = run_git(&["config", &format!("branch.{}.merge", branch)], &cache).unwrap();
        assert_eq!(remote_cfg, "origin");
        assert_eq!(merge_cfg, format!("refs/heads/{}", branch));
    }

    #[test]
    fn migrate_refs_to_remotes_handles_multiple_branches() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "x").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();

        let cache = td.path().join("cache.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();
        let default = default_branch(&cache).unwrap();

        // Push two extra branches from the seed clone, then re-clone the
        // cache so it picks up all three branches in refs/heads/*.
        for b in ["feature-a", "feature-b"] {
            run_git(&["checkout", "-b", b], &seed).unwrap();
            push_to_branch(&seed, b).unwrap();
            run_git(&["checkout", &default], &seed).unwrap();
        }
        let cache = td.path().join("cache2.git");
        clone_bare(bare_remote.to_str().unwrap(), &cache).unwrap();

        for b in [&default, "feature-a", "feature-b"] {
            let remote_ref = format!("refs/remotes/origin/{}", b);
            assert!(
                !ref_exists(&cache, &remote_ref).unwrap(),
                "tracking ref for {} should be absent before migration",
                b
            );
        }

        migrate_refs_to_remotes(&cache).unwrap();
        for b in [&default, "feature-a", "feature-b"] {
            let remote_ref = format!("refs/remotes/origin/{}", b);
            assert!(
                ref_exists(&cache, &remote_ref).unwrap(),
                "tracking ref for {} should exist after migration",
                b
            );
        }
    }

    #[test]
    fn sync_refs_from_updates_tracking_refs_and_missing_tags() {
        let td = TempDir::new().unwrap();
        let bare_remote = td.path().join("remote.git");
        init_bare(&bare_remote);

        let seed = td.path().join("seed");
        clone(bare_remote.to_str().unwrap(), &seed, None).unwrap();
        init_repo(&seed);
        fs::write(seed.join("f.txt"), "v1").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "init").unwrap();
        push(&seed).unwrap();
        // Tag on the first commit, present everywhere from the start.
        run_git(&["tag", "v1.0"], &seed).unwrap();
        run_git(&["push", "origin", "v1.0"], &seed).unwrap();

        let shared = td.path().join("shared.git");
        clone_bare(bare_remote.to_str().unwrap(), &shared).unwrap();
        fetch_all(&shared).unwrap();
        let isolated = td.path().join("isolated.git");
        clone_bare_from_shared(&shared, &isolated, bare_remote.to_str().unwrap()).unwrap();
        let branch = default_branch(&isolated).unwrap();

        // A tag that exists locally at the first commit; the remote will move
        // the same tag name to the second commit — sync must not clobber it.
        let first_commit = head_rev(&bare_remote).unwrap();
        run_git(&["tag", "moved", &first_commit], &isolated).unwrap();

        // Advance the remote: new commit + new tag.
        fs::write(seed.join("f.txt"), "v2").unwrap();
        add_all(&seed).unwrap();
        commit(&seed, "second").unwrap();
        push(&seed).unwrap();
        run_git(&["tag", "v2.0"], &seed).unwrap();
        run_git(&["push", "origin", "v2.0"], &seed).unwrap();
        // Move the remote's "moved" tag to a different commit.
        run_git(&["tag", "-f", "moved", "HEAD"], &seed).unwrap();
        run_git(&["push", "origin", "moved", "--force"], &seed).unwrap();

        // Only the shared pool fetches from the remote.
        fetch_all(&shared).unwrap();
        sync_refs_from(&isolated, &shared).unwrap();

        // Tracking ref updated to the new remote head.
        let remote_head = head_rev(&bare_remote).unwrap();
        let synced = run_git(
            &["rev-parse", &format!("refs/remotes/origin/{}", branch)],
            &isolated,
        )
        .unwrap();
        assert_eq!(synced, remote_head);

        // New tag created; pre-existing diverged tag left untouched.
        assert!(ref_exists(&isolated, "refs/tags/v2.0").unwrap());
        let moved = run_git(&["rev-parse", "refs/tags/moved"], &isolated).unwrap();
        assert_eq!(
            moved, first_commit,
            "existing local tag must not be clobbered"
        );
    }
}
