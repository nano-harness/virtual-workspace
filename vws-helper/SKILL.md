---
name: vws-helper
description: >
  Use this skill whenever the user or agent needs to manage one or more virtual workspaces with vwsd/vws.
  vwsd prepares real, git-backed workspace directories: remote git sources become real `git worktree`
  directories created directly under the workspace mountpoint, and local directory sources are used in
  place (auto `git init` if needed). There is no NFS/FUSE mount, no symlink, no copy-on-write overlay.
  Trigger on keywords: vwsd, vws, virtual workspace, workspace mount, workspace unmount, workspace config,
  multi-workspace, parallel workspaces, workspace management, mounting git repos, workspace init,
  workspace doctor, workspace list, git worktree workspace.
---

# vws-helper 技能

本技能指导如何安装 `vwsd`、创建工作区配置并管理真实目录形式的工作区。

## ⚠️ 架构说明（先读这里）

`vwsd` 不再是 NFS/FUSE 虚拟文件系统工具。它只负责在磁盘上准备**真实的 Git 目录**：

- **远程 Git 源**：先缓存为 `~/.vws/git_cache/` 下的 bare repo，再直接在 `<mountpoint>/<mount_as>` 位置用 `git worktree add` 创建一个**真实**的工作目录（不是 symlink，`cd`/`find`/IDE 索引都是普通本地目录）。
- **本地目录源**：始终**原地使用**，不复制、不建 symlink 到 mountpoint。如果该目录还不是 Git 仓库，`vwsd mount` 会自动在里面执行 `git init`；如果失败（如无权限），只会告警并跳过该源，不会导致整个工作区挂载失败。
- **没有**：NFS/FUSE 挂载、upperdir、COW（写时复制）、虚拟合并文件系统、后台 daemon、watchdog、端口、锁模式。`mount` 是一次性的同步操作：为每个源准备好真实目录后立即退出，没有需要保活的进程。
- 因此一个混合工作区实际上是**两类真实位置的集合**：远程源在 `mountpoint/<mount_as>` 下，本地源仍在原始 `path`。`vwsd list`/`vwsd status` 会如实报告每个源的真实磁盘路径，不假装它们都在同一目录下。

## 快速入门分层

### 零门槛：纯本地目录

无需任何 git URL，只需一条命令，通过交互式向导配置即可：

```bash
vwsd init
# 按提示选择模板或手动配置，完成后运行 `vwsd mount --config ...`
```

### 低门槛：AI Agent 直接生成配置

AI Agent 可以直接写入 JSON 配置文件，用户无需手动提供任何 URL：

1. AI 通过 `gh repo view` 自动查询仓库的 git clone URL。
2. AI 直接将 JSON 写入 `~/.vws/workspaces/<名称>.json`。
3. 用户执行 `vwsd mount --config ~/.vws/workspaces/<名称>.json`。

完整流程见下文"AI Agent 自动化流程"章节。

### 标准用法：本地目录 + 远程仓库混合

手动编写配置文件，详见下文"配置文件格式"章节。

---

## ⚠️ AI Agent 非交互操作要点

`vwsd` 的部分命令默认是**交互式**的（菜单选择、确认提示）。在 AI Agent / 脚本 / CI 中使用时**必须显式给出 `--name`/`--config` 参数与非交互标志**，否则进程会阻塞在提示上。

| 目的 | 非交互命令 |
|------|-----------|
| 创建工作区 | `vwsd init --name <名称> --source <路径或git URL> [--source ...] --json` |
| 准备工作区（同步执行，立即返回） | `vwsd mount --config ~/.vws/workspaces/<名称>.json [--json]` |
| 查看状态 | `vwsd list --json`（加 `--fetch` 先抓取远程再计算 Git 状态） |
| 同步远程更新 | `vwsd sync --name <名称> --json` |
| 推送本地提交 | `vwsd push --name <名称> [--yes] --json`（不带 `--yes` 仅预览） |
| 卸载（删除远程 worktree，本地源不动） | `vwsd umount --name <名称> --force` |
| 销毁（不可逆，额外清理配置/bare cache） | `vwsd destroy --name <名称> --force` |
| 诊断与修复 | `vwsd doctor [--json] [--fix]`、`vwsd repair --json` |
| 生成 IDE 工作区文件 | `vwsd vscode --name <名称>` |

规则：

- 永远指定 `--name` 或 `--config`，不要依赖交互式选择菜单。
- `umount` / `destroy` 必须带 `--force` 才不会提示；执行 `destroy` 前必须先与用户确认（不可逆）。
- 需要结构化输出时加 `--json`（`init`/`mount`/`restart`/`list`/`status`/`doctor`/`sync`/`push`/`repair` 均支持；`mount --json` 输出每个源的 `ok`/`warn` 结果数组）。
- vwsd 没有后台 daemon，因此**没有 logs 命令**；排查问题优先用 `vwsd doctor --config <配置> --json`，它会报告 git 环境、配置合法性、本地源是否为 git 仓库、远程 worktree 是否存在、bare cache 是否健康。

---

## 核心概念

- **配置文件**：每个工作区一个 JSON 文件，位于 `~/.vws/workspaces/<工作区名称>.json`。
- **`mountpoint`**：一个普通目录，只用来存放**远程源**的真实 `git worktree`；本地源不会出现在这里。
- **`mount_as`**：远程源在 mountpoint 下的目录名。必须是挂载点内部的相对路径（纯目录名，可嵌套如 `group/repo`），不允许绝对路径、`.`、`..` 等会逃逸挂载点的写法，否则配置会被拒绝加载。
- **`id`**：工作区的持久化 UUID v4 身份标识。新配置由 `vwsd init` 自动生成；旧版（含已废弃字段的）配置首次 `vwsd mount` 时会自动补全 `id` 并回写。
- **安装方式**：使用 Rust 工具链从源码构建。

## Git 挂载机制

- **Bare + Worktree 架构**：远程仓库先 `git clone --bare` 到共享缓存 `~/.vws/git_cache/<key>.git`（多个工作区共享同一 bare 缓存以节省磁盘和网络），再用 `git worktree add` 为每个工作区在 `<mountpoint>/<mount_as>` 创建一个**独立、真实**的工作目录。
- **分支模式**：worktree 以正常分支模式运行（非 detached HEAD），可以直接 `git commit`，变更天然持久化到本地分支。
- **重新挂载**：再次 `vwsd mount` 时会自动 `git fetch` 获取远程更新，但不会自动 rebase/merge，由用户决定何时合并（`git pull --rebase` 或 `vwsd sync`）。
- **自动本地分支创建**：当配置的 `branch` 在远程不存在时，`vwsd` 会自动基于仓库默认分支（如 main/master）创建本地分支，无需先在远程创建该分支。

## 构建与安装

```bash
git clone https://github.com/nano-harness/virtual-workspace.git
cd virtual-workspace
cargo build --release
mkdir -p ~/.local/bin
install -m 755 ./target/release/vwsd ~/.local/bin/vwsd
```

请确保 `~/.local/bin` 已加入 `PATH`。

## `vwsd` 命令用法

`vwsd --help` / `vwsd <子命令> --help` 可随时查看完整参数摘要。

### `init` - 创建工作区配置

交互式向导（模板选择或手动分步配置）：

```bash
vwsd init
```

非交互模式（适合脚本与 AI Agent），`--source` 可多次指定，本地路径或 git URL 均可：

```bash
vwsd init --name my-workspace --source ~/code/project-a --source git@github.com:owner/repo.git --json
```

- `--name`：工作区名称；`--mountpoint`：可选，覆盖默认挂载点路径。
- `--template`：`basic`（默认）/ `frontend` / `rust` / `maven` / `monorepo`。仅交互式向导使用；非交互模式下该参数会被解析但不起作用。
- `--vscode`：同时生成 `.code-workspace` 文件；`--json`：打印生成的配置 JSON。
- 非交互模式至少需要一个 `--source`；不带任何参数时进入交互式向导。

### `mount` - 准备工作区（同步操作）

为配置中的每个源准备好真实目录，然后立即退出——不会启动任何后台进程或挂载文件系统。

```bash
# 交互式选择（列出所有已配置的工作区）
vwsd mount

# 指定配置文件
vwsd mount --config ~/.vws/workspaces/my-workspace.json

# 按名称查找配置
vwsd mount --name my-workspace

# 挂载全部已配置的工作区（单个失败不会中断其余工作区）
vwsd mount --all

# 结构化输出（每个源一条 ok/warn 结果）
vwsd mount --config ~/.vws/workspaces/my-workspace.json --json
```

- 远程源：确保 bare cache 存在（必要时 `git fetch`），再在 `<mountpoint>/<mount_as>` 创建/复用真实 `git worktree`。
- 本地源：确认目录是 git 仓库；不是则自动 `git init`；失败则告警并跳过该源（其余源仍会继续挂载）。
- 命令输出会逐条列出每个源的处理结果（`ok` 或 `warn`），可用于判断哪些源需要人工介入。

### `list` / `status` - 查看工作区状态

```bash
vwsd list            # 人类可读输出
vwsd list --json     # 结构化输出（AI Agent 推荐）
vwsd list --fetch    # 先 git fetch 再计算 ahead/behind
```

输出中每个 source 都带 `kind`（`local`/`remote`）和 `path`（该源的真实磁盘路径——本地源是原始 `path`，远程源是 `<mountpoint>/<mount_as>`），以及分支同步状态（`state` 字段：`ready`/`dirty`/`unpushed`/`behind`/`git`/`not_git`/`missing`）。`vwsd status` 是 `list` 的别名，参数与输出完全相同。

### `sync` / `push` - 同步与推送 Git 源

```bash
vwsd sync --name my-workspace            # 对配置了 origin 且无本地风险的源执行 fetch + rebase
vwsd push --name my-workspace            # 仅报告待推送的 commit（不实际推送）
vwsd push --name my-workspace --yes      # 实际执行推送
```

两者都支持 `--json`。有未提交变更或未推送 commit 时 `sync` 会跳过该源；没有 `origin` 远程的源也会被跳过。

### `umount` / `stop` - 卸载远程 worktree

删除远程源的 `git worktree`；**本地源不受任何影响**（本来就没有被移动或复制）。

```bash
vwsd umount --name my-workspace --force
```

- 卸载前会检查每个远程 worktree 是否有未提交变更 / 未推送 commit，并给出警告。**注意：未提交的变更只存在于 worktree 工作区中，删除 worktree 后会永久丢失**，bare 缓存里不会有；只有已提交但未推送的 commit 保留在隔离 bare 仓库（`~/.vws/git_cache/<key>__<工作区名>.git`）的分支上，下次 `mount` 重建 worktree 后仍可用。只有在警告后确认继续（或 `--force`）才会真正删除 worktree 目录。
- `--all`：卸载所有已配置工作区。

### `destroy` - 彻底删除工作区

删除远程 worktree、删除工作区配置文件、按需清理不再被其他工作区使用的 bare cache、删除 mountpoint 目录。**不会删除任何本地源目录**——本地源路径本来就是用户自己的目录，`vwsd` 从不代管它的生命周期。如果某个本地源目录位于 mountpoint 内部，`destroy` 会保留 mountpoint 并提示，而不是递归删除。

```bash
vwsd destroy --name my-workspace --force
```

### umount vs destroy 对比

| 操作 | 删除 | 保留 | 可逆 |
|------|------|------|------|
| `vwsd umount` | 远程 worktree 目录 | bare cache、配置文件、本地源目录 | ✅ 再次 `vwsd mount` 会重建 worktree |
| `vwsd destroy` | 远程 worktree、配置文件、mountpoint 目录、（若无人使用）bare cache | 本地源目录 | ❌ 不可逆 |

### `restart` - 刷新工作区

`vwsd restart [--name|--config|--all]`：重新执行一遍挂载/刷新逻辑（相当于对每个源重新 `git fetch`/确保 worktree 存在），不再有"重启后台进程"的含义，因为已经没有常驻进程。

### `doctor` / `repair` - 环境检查与修复

```bash
vwsd doctor              # 检查 git 环境、配置合法性、本地源是否为 git 仓库、远程 worktree/bare cache 是否健康
vwsd doctor --fix        # 先应用安全自动修复（等同 vwsd repair），再输出报告
vwsd doctor --json       # 结构化输出，含 environment 检查与 summary.{passed,warnings,failures}
vwsd repair --json       # 创建缺失的 mountpoint 目录、补全缺失的 workspace id 等
```

### `vscode` - 生成 IDE 工作区文件

```bash
vwsd vscode --name my-workspace
```

生成一个多根 `.code-workspace` 文件，为每个源（本地路径 + 远程 worktree 路径）各添加一个 folder 条目，包含绝对路径。

### 其他命令

- `vwsd start` / `vwsd stop`：分别是 `vwsd mount` / `vwsd umount` 的别名。
- `vwsd completion <bash|zsh|fish|powershell|elvish>`：生成 shell 补全脚本。

## 配置文件管理

- 配置文件是理解和调试工作区行为的关键，位于 `~/.vws/workspaces/<名称>.json`。
- 可以手动编辑（修改前先备份）。常见修改：更改 `mountpoint`、修改 `remote_git.url` 或 `branch`、增删 `sources`。
- **向后兼容**：旧版（NFS 架构时代）配置文件里包含的 `mode`/`upperdir`/`nfs_port`/`lock_mode`/`filters` 以及 source 级别的 `priority`/`access` 字段已废弃，但 `vwsd` 会静默忽略这些未知字段，旧配置文件可以直接拿来 `vwsd mount`，无需手动迁移；`vwsd mount` 首次处理时会自动补全 `id` 并回写一份不含废弃字段的精简配置。

## 配置文件格式

以下是一个混合了本地目录和远程 Git 仓库的配置示例（当前格式）。

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "name": "mixed-workspace",
  "mountpoint": "~/VirtualWorkspaces/mixed-workspace",
  "sources": [
    {
      "path": "~/Projects/local-code",
      "mount_as": "local-code"
    },
    {
      "path": "",
      "mount_as": "shared-lib",
      "remote_git": {
        "url": "https://github.com/user/shared-lib.git",
        "branch": "main"
      }
    }
  ]
}
```

### 关键字段说明

- `id`：工作区持久化身份标识（UUID v4）。新配置由 `vwsd init` 自动生成；旧配置首次挂载时自动补全。
- `name`：工作区名称。
- `mountpoint`：存放**远程源**真实 worktree 的目录；本地源不会出现在这里。
- `sources`：定义工作区内容的来源，是一个对象数组。
    - `path`：本地源的真实路径（该目录会被原地使用，非 git 仓库时自动 `git init`）。远程 Git 源此字段留空字符串。
    - `mount_as`：此源的名称——对远程源是它在 `mountpoint` 下的真实子目录名；对本地源仅作为显示标签，不影响其实际路径。
    - `remote_git`：如果是远程 Git 源，此对象包含 Git 相关信息。
        - `url`：Git 仓库的 URL。
        - `branch`：要使用的分支。如果分支在远程已存在，直接使用该分支；如果不存在，`vwsd` 会自动基于仓库默认分支创建本地分支，之后可直接 `git push` 推送到远程建立该分支。

## AI Agent 自动化流程

AI Agent 可以全程自动完成"查 URL → 生成配置 → 挂载"的完整流程，无需用户手动操作。

### 第一步：自动获取仓库 git clone URL

```bash
gh repo view <owner>/<repo> --json sshUrl,httpsUrl
```

从输出中提取 `git clone` URL（优先使用 SSH URL，失败则使用 HTTPS URL）。

### 第二步：AI 直接生成并写入配置文件

AI 根据用户意图，直接将 JSON 配置写入 `~/.vws/workspaces/<名称>.json`，无需 `vwsd init`。

**示例：开发单个远程仓库**

```json
{
  "name": "my-repo",
  "mountpoint": "~/VirtualWorkspaces/my-repo",
  "sources": [
    {
      "path": "",
      "mount_as": "my-repo",
      "remote_git": {
        "url": "git@github.com:owner/my-repo.git",
        "branch": "main"
      }
    }
  ]
}
```

**示例：合并多个本地项目（零门槛，无需 git URL）**

```json
{
  "name": "local-projects",
  "mountpoint": "~/VirtualWorkspaces/local-projects",
  "sources": [
    { "path": "~/code/project-a", "mount_as": "project-a" },
    { "path": "~/code/project-b", "mount_as": "project-b" }
  ]
}
```

本地源即使还不是 git 仓库也没关系，`vwsd mount` 会自动执行 `git init`。联合多个远程仓库时同理：`sources` 数组中为每个仓库添加一个带 `remote_git` 的条目即可。

### 第三步：挂载

```bash
vwsd mount --config ~/.vws/workspaces/<名称>.json
```

命令同步执行并立即返回，输出中会列出每个源的处理结果。

---

## 常见场景速查

| 用户说 | AI 应做什么 |
|--------|------------|
| "帮我创建一个工作区开发 xxx 仓库" | 1. `gh repo view` 查 clone URL<br>2. 生成 JSON 配置写入 `~/.vws/workspaces/xxx.json`<br>3. 执行 `vwsd mount --config ...` |
| "把我本地的几个项目合并到一个工作区" | `vwsd init --name <名称> --source ~/code/a --source ~/code/b`，再 `vwsd mount --config ...` |
| "帮我创建一个包含 A 和 B 仓库的联合工作区" | 查两个仓库的 URL，生成多 `sources` 的 JSON 配置，挂载 |
| "帮我在 xxx 仓库开一个新功能分支" | 生成配置时 `branch` 填写新分支名（如 `feature/xxx`），`vwsd` 自动基于默认分支创建 |
| "工作区现在什么状态？" | `vwsd list --json`（需要最新远程状态时加 `--fetch`） |
| "工作区打不开了，帮我看看状态和日志" | vwsd 没有 daemon，也没有 logs 命令；用 `vwsd list --json` 看状态、`vwsd doctor --json` 诊断，可修复问题用 `vwsd doctor --fix` |
| "挂载/准备失败了" | 看 `vwsd mount` 的输出（每个源都有 `ok`/`warn` 结果）；`vwsd doctor --json` 进一步诊断；必要时 `vwsd repair` |
| "帮我把本地改动推上去" | `vwsd push --name <名称>` 先预览，确认后 `vwsd push --name <名称> --yes` |
| "拉一下远程最新代码" | `vwsd sync --name <名称>` |
| "这个工作区不用了" | 删除远程 worktree 用 `vwsd umount --name <名称> --force`；彻底清理（先与用户确认，本地源不受影响）用 `vwsd destroy --name <名称> --force` |
| "我有个老的 workspace 配置文件（带 upperdir/nfs_port 字段）还能用吗" | 能，`vwsd` 会忽略这些废弃字段，直接 `vwsd mount --config <路径>` 即可；首次挂载会自动补全 `id` 并回写精简后的配置 |

---

## 工作流程示例

1. **创建新工作区**：`vwsd init`（或非交互 `vwsd init --name ... --source ...`）
2. **准备工作区**：`vwsd mount --config ~/.vws/workspaces/my-new-project.json`
3. **在各个源目录中工作**：
   - 远程 Git 源的 worktree 以分支模式运行（非 detached HEAD），可直接 `git commit`，提交自动持久化到本地分支。
   - 本地源就是原始目录本身，无需任何额外步骤。
4. **工作结束后卸载远程 worktree**（本地源不受影响）：`vwsd umount --name my-new-project --force`
5. **再次挂载时**，`vwsd` 会自动执行 `git fetch` 获取远程更新；用 `git pull --rebase` 或 `vwsd sync` 手动合并变更。

### 新分支开发

当需要在远程仓库上开始一个新功能分支的开发时，只需在配置中指定分支名，即使该分支在远程不存在：

```json
{
  "remote_git": {
    "url": "https://github.com/user/repo.git",
    "branch": "feature/my-new-feature"
  }
}
```

`vwsd` 会自动基于仓库的默认分支创建本地分支 `feature/my-new-feature`。在 worktree 中开发后，可直接使用 `git commit` 和 `git push` 将变更推送到远程。

## 多工作区并行

每个工作区都是相互独立的：`mount`/`umount`/`sync` 等命令都是一次性的同步操作，不涉及端口分配或进程管理，可以对任意多个工作区自由并行操作而不会相互冲突（远程仓库的 bare cache 会在多个工作区之间共享，但各自的 worktree 是隔离的）。

```bash
vwsd mount --config ~/.vws/workspaces/ws-a.json
vwsd mount --config ~/.vws/workspaces/ws-b.json
```
