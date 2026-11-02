# Virtual Workspace (vws)

[English](./README.md) | [简体中文](./README.zh-CN.md)

`vwsd` 现在只负责准备**真实的 Git 目录**。

- 远程 Git 源会先缓存为 `~/.vws/git_cache/` 下的 bare repo，再直接在 `<mountpoint>/<mount_as>` 位置创建真实 `git worktree` 目录。
- 本地路径源始终**原地使用**。如果目录还不是 Git 仓库，`vwsd mount` 会在该目录内执行 `git init`。如果失败，只会明确告警并跳过该源，不会让整个工作区失败。
- 不再有 NFS/FUSE、upperdir、COW、虚拟合并文件系统、后台 daemon、watchdog、端口或锁模式。

## 工作原理

工作区里的 `mountpoint` 现在只是普通目录：

- 远程源会真实出现在这个目录下面；
- 本地源继续保留在原始路径，并由 `vwsd status` / `vwsd list` 直接报告真实磁盘路径。

因此，混合工作区会同时包含两类真实位置：
一部分在 `mountpoint` 下（远程 worktree），另一部分仍在原目录（本地仓库）。

## 安装

### 源码编译（推荐）

```bash
git clone https://github.com/nano-harness/virtual-workspace.git
cd virtual-workspace
cargo build --release
mkdir -p ~/.local/bin
install -m 755 ./target/release/vwsd ~/.local/bin/vwsd
```

请确保 `~/.local/bin` 已加入 `PATH`。

## 快速开始

```bash
vwsd init \
  --name my-ws \
  --source ~/Projects/app \
  --source https://github.com/user/shared-lib.git

vwsd mount --config ~/.vws/workspaces/my-ws.json
vwsd status
```

## 配置

配置文件位于 `~/.vws/workspaces/<名称>.json`。本地与远程混合工作区示例：

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

`mount_as` 可选，默认为仓库名；`id` 会自动生成。本地源使用 `path`；远程源使用空 `path`、`remote_git.url` 和可选的 `remote_git.branch`。

## 命令说明

- `vwsd mount` / `vwsd start`：同步准备所有源，然后立即退出。
- `vwsd umount` / `vwsd stop`：删除远程 worktree；本地源不做任何改动。
- `vwsd destroy`：删除远程 worktree、删除工作区配置、按需清理无人使用的 bare cache，并删除工作区 mountpoint 目录；**不会删除本地源目录**。
- `vwsd sync`：对干净且配置了 `origin` 的仓库执行 fetch/rebase。
- `vwsd push --yes`：推送干净仓库里的未推送提交。
- `vwsd doctor`：检查配置、本地仓库状态、远程 worktree 是否存在、bare cache 是否健康。
- `vwsd repair`：创建缺失的 mountpoint 目录并补全缺失的 workspace ID。
- `vwsd vscode`：生成包含绝对路径的 `.code-workspace` 文件。
- `vwsd completion`：生成 shell 补全脚本。

## 关于本地源

本地源不会被复制到 mountpoint，也不会在那里创建 symlink。真正的权威路径始终是配置里的 `sources[].path`。

这是刻意的设计：既然已经没有旧的虚拟文件系统层，就不再伪装成本地源“也挂载到了 mountpoint 里”。

## 构建与测试

CI 目前执行：

```bash
cargo fmt --all -- --check
cargo test
```

建议本地验证：

```bash
cargo fmt
cargo build
cargo test
cargo clippy --all-targets
```
