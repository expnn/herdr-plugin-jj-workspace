# Proposal: lifecycle-docs

## Why

两个独立的收尾项：

1. **生命周期语义从未成体系地文档化**。jj workspace 是同一仓库的多个工作副本：commit/bookmark 层在所有工作副本间即时共享（副工作区的 `.jj/repo` 是文件指针），只有磁盘文件物化彼此隔离——这正是 workspace 的意义，但用户无法从现有 README 推断出"修改会自动同步回主仓库吗"的答案（不会，也不该做：回主线是手动 jj 工作流）。
2. **`remove` 是唯一真实的数据丢失点**。实测证据（jj 0.45.1）：`jj workspace forget` 对脏工作副本（未提交修改）**静默成功、无任何保护**；脏 commit 以匿名形式存活于共享存储，但物化文件随后被插件 `rm -rf` 删除——从用户视角，未提交的工作丢失了。而已提交内容（含书签）经实验确认在 forget 后完整存活，无需为它们警告。

## What Changes

- **Part A（文档）**：README 新增 "How jj workspaces relate" 章节——commit 层共享 / 工作副本隔离 / 回主线为手动 jj 工作流 / remove 语义（主工作区永不删除、脏 @ 拒绝）。
- **Part B（行为）**：`cmd_remove` 在 `jj workspace forget` 之前检查 `jj diff --summary -r @`；非空（存在未提交修改）→ **拒绝执行**（stderr 指明原因与出路：先 `jj commit` 或 `jj restore`），不做任何删除动作。
- 不新增配置键：headless action 没有带参重试通道，拒绝消息指向的手动路径永远可用，比"警告后继续放行"诚实。
- 范围外：未推送书签检测（实验证明书签在 forget 后存活，警告是噪音）、自动 stash、确认式交互（headless 无 TTY）、任何自动同步/自动 merge（既定出 scope）。

## Impact

- Affected specs: `workspace-removal`（NEW 能力：脏 @ 拒绝）
- Affected code: `cmd_remove`（一个 `jj` 调用 + 一个 die 分支）；README（一个新章节）
- 依赖：config-foundation / jj-path-resolution 已实施（`load_config` / 已解析的 `jj.command` 直接复用）
