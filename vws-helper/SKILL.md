---
name: vws-helper
description: Create and manage multi-repository workspaces with vwsd. Use for virtual workspaces, Git worktree setup, local and remote workspace sources, workspace mount/unmount, configuration, status, synchronization, push preview, diagnostics, and editor workspace files.
license: MIT-0
metadata:
  openclaw:
    requires:
      bins:
        - vwsd
        - git
    homepage: https://github.com/nano-harness/virtual-workspace
---

# vws-helper

Use `vwsd` to prepare real directories for workspaces containing local directories and remote Git repositories.

- Remote sources are cached as bare repositories and checked out as real Git worktrees under `<mountpoint>/<mount_as>`.
- Local sources remain at their original paths and are used in place. A local non-Git directory is initialized with `git init` during `mount`; failure produces a warning and skips that source.
- There is no NFS/FUSE filesystem, background daemon, or merged virtual directory. `mount` prepares directories and exits.

## Install from source

Requires a current stable Rust toolchain and Git:

```bash
git clone https://github.com/nano-harness/virtual-workspace.git
cd virtual-workspace
cargo build --release
mkdir -p ~/.local/bin
install -m 755 ./target/release/vwsd ~/.local/bin/vwsd
```

Ensure `~/.local/bin` is on `PATH`.

## Non-interactive workflow

For agents and scripts, always pass `--name` or `--config` instead of relying on interactive selection.

1. Create the workspace:

   ```bash
   vwsd init \
     --name my-ws \
     --source ~/Projects/app \
     --source https://github.com/user/shared-lib.git \
     --json
   ```

2. Prepare the directories:

   ```bash
   vwsd mount --config ~/.vws/workspaces/my-ws.json --json
   ```

3. Inspect actual source paths and Git state:

   ```bash
   vwsd list --json
   ```

`vwsd status` is an alias for `list`. Add `--fetch` when remote ahead/behind information must be refreshed.

## Configuration

Configs live in `~/.vws/workspaces/<name>.json`. A mixed workspace looks like this:

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

- Local sources use `path`.
- Remote sources use an empty `path` and `remote_git.url`; `remote_git.branch` is optional.
- `mount_as` defaults to the repository name and must be a safe relative path.
- `id` is generated automatically.
- A missing configured branch is created locally from the repository's default branch.

Agents may use `gh repo view <owner>/<repo> --json sshUrl,httpsUrl` to discover a URL when `gh` is available; it is not a `vwsd` dependency. Prefer `vwsd init` over manually writing config when its prompts are sufficient.

## Command reference

| Goal | Command |
|---|---|
| Create a config interactively | `vwsd init` |
| Create a config non-interactively | `vwsd init --name <name> --source <path-or-url> --json` |
| Prepare one workspace | `vwsd mount --name <name> --json` |
| Prepare every configured workspace | `vwsd mount --all` |
| Show workspace state | `vwsd list --json` |
| Fetch and rebase clean sources with `origin` | `vwsd sync --name <name> --json` |
| Preview unpushed commits | `vwsd push --name <name> --json` |
| Push after user confirmation | `vwsd push --name <name> --yes --json` |
| Diagnose configs, worktrees, and caches | `vwsd doctor --json` |
| Apply safe repairs | `vwsd repair --json` |
| Generate an editor workspace | `vwsd vscode --name <name>` |
| Remove remote worktrees | `vwsd umount --name <name> --force` |
| Delete workspace config and remote artifacts | `vwsd destroy --name <name> --force` |
| Generate shell completions | `vwsd completion zsh` |

`start` and `stop` are aliases for `mount` and `umount`. There is no `logs` command; use `list` and `doctor` for diagnostics.

## Safety rules

- Inspect `vwsd list --json` before destructive operations.
- `umount` deletes remote worktrees but leaves local source directories untouched. Uncommitted worktree changes can be lost permanently; committed changes may remain in the cache.
- `destroy` additionally deletes the workspace config, selected caches, and possibly the mountpoint. It is irreversible and requires explicit user confirmation.
- Do not use `--force` to bypass a warning the user has not reviewed.
- `sync` skips sources with local risks or without `origin`; report skipped sources rather than claiming they were updated.
- `push` without `--yes` is a preview. Only `--yes` performs network publication.
- Do not place credentials in workspace JSON, command output, or agent summaries.

## Agent guidelines

- Use `--json` for machine-readable results.
- Report the actual path of every source; do not claim local sources moved into the mountpoint.
- Use `doctor --json` before suggesting repairs and `doctor --fix` only for the documented safe fixes.
- Confirm destructive operations before running them, and report partial per-source failures accurately.
