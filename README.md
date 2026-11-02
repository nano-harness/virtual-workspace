# Virtual Workspace (vws)

[English](./README.md) | [简体中文](./README.zh-CN.md)

`vwsd` prepares **real directories backed by Git**.

- Remote Git sources are cached as bare repos under `~/.vws/git_cache/` and checked out as real `git worktree` directories directly at `<mountpoint>/<mount_as>`.
- Local path sources are used **in place**. If a local directory is not already a Git repo, `vwsd mount` runs `git init` there. If that fails, the source is skipped with a warning instead of failing the whole workspace.
- There is no NFS/FUSE layer, no copy-on-write upperdir, no virtual merge filesystem, and no background daemon.

## How it works

A workspace config still has a `mountpoint`, but it is now just a normal directory:

- remote sources materialize inside it as real subdirectories;
- local sources stay at their original paths and are reported honestly by `vwsd status` / `vwsd list`.

This means mixed workspaces are partly under the mountpoint (remote worktrees) and partly in-place (local repos).

## Installation

### Build from source (recommended)

```bash
git clone https://github.com/nano-harness/virtual-workspace.git
cd virtual-workspace
cargo build --release
mkdir -p ~/.local/bin
install -m 755 ./target/release/vwsd ~/.local/bin/vwsd
```

Ensure `~/.local/bin` is on your `PATH`.

## Quick start

```bash
vwsd init \
  --name my-ws \
  --source ~/Projects/app \
  --source https://github.com/user/shared-lib.git

vwsd mount --config ~/.vws/workspaces/my-ws.json
vwsd status
```

## Configuration

Configurations are stored in `~/.vws/workspaces/<name>.json`. A mixed local/remote workspace looks like this:

```json
{
  "name": "my-ws",
  "mountpoint": "~/VirtualWorkspaces/my-ws",
  "sources": [
    { "path": "~/Projects/app" },
    {
      "path": "",
      "remote_git": {
        "url": "https://github.com/user/shared-lib.git",
        "branch": "main"
      },
      "mount_as": "shared-lib"
    }
  ]
}
```

`mount_as` is optional and defaults to the repository name. `id` is generated automatically. Local sources always use `path`; remote sources use an empty `path` plus `remote_git.url` and an optional `remote_git.branch`.

## Commands

- `vwsd mount` / `vwsd start`: synchronously prepare all sources and exit.
- `vwsd umount` / `vwsd stop`: remove remote worktrees; local sources are untouched.
- `vwsd destroy`: remove remote worktrees, delete the workspace config, optionally clean unused bare caches, and remove the workspace mountpoint directory. Local source directories are **not** deleted.
- `vwsd sync`: fetch/rebase clean repositories that have an `origin` remote.
- `vwsd push --yes`: push clean repositories with unpushed commits.
- `vwsd doctor`: validate config, local repo readiness, worktree presence, and bare-cache health.
- `vwsd repair`: create missing mountpoint directories and assign missing workspace IDs.
- `vwsd vscode`: generate a `.code-workspace` file with absolute folder paths.
- `vwsd completion`: generate shell completion scripts.

## Notes on local sources

Local sources are not copied into the mountpoint and are never symlinked there. The authoritative path remains the original `sources[].path` directory.

That behavior is intentional: without the old virtual filesystem layer, pretending local sources live under the mountpoint would be misleading.

## Build and test

CI runs:

```bash
cargo fmt --all -- --check
cargo test
```

Recommended local validation after changes:

```bash
cargo fmt
cargo build
cargo test
cargo clippy --all-targets
```
