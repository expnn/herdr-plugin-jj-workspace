# Proposal: opencode-v2-session-migration

## Why

删除 jj workspace 前的 opencode session 迁移只枚举 v1 表 `session`，而 opencode v2（本机 2.0.29）把活跃 session 只写入 `session_v2`，v1 `session` 已冻结为历史表。结果是删除对话框对 v2 session 静默显示「no opencode sessions bound to this workspace」（Skipped），用户看不到任何可迁移 session；若继续删除，这些 session 的 `directory` 将指向已删目录，成为孤儿。实测待删 workspace `/home/cyc/Workspace/WaveletsGen/workspace-upgrade-frontend` 有 13 条 session 全在 `session_v2`，`session` 表中为 0 条。

## What Changes

- 迁移与预览按 opencode 数据版本选择**活跃 session 表**：存在 `session_v2` 表时以它为唯一枚举/更新目标，否则沿用 `session`（v1 兼容）。
- 目标 `project_id` 解析按版本分派：v2 以 `project.worktree = canonical(主仓库根)` 精确匹配（0 行或多行均拒绝）；v1 保持既有链（纯 jj → `global`、`.git/opencode` 缓存、按 `worktree` 查询）。
- 写前 schema 探测针对**所选活跃表**；必需列缺失即拒绝删除（fail-closed），不降级执行。
- 收紧「跳过 vs 拒绝」边界：活跃表存在但 schema 不符（含未来版本变化）→ 拒绝删除；仅「opencode/DB 发现缺失」与「枚举计数为 0」保持静默跳过。
- 列语义继续对齐 opencode 官方 `SessionEvent.Moved` projector：`project_id`、`directory`、`path`（主仓库根下为空串）、`workspace_id`（NULL）、`time_updated`（当前毫秒）。
- v2 下不删除 `project` / `worktree` 行（交由 opencode 自身 `Project.sweep` / worktree 服务）；v1 路径保留既有 `project_directory` 清理。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `opencode-session-migration`：活跃 session 表选择（v1/v2）、v2 目标 project 解析、schema 探测对象、跳过与拒绝的边界。

## Impact

- `src/opencode_migration.rs`：活跃表选择与探测、枚举/计数/UPDATE 的 SQL 目标表、`resolve_target` 版本分派、v2 清理策略。
- `openspec/specs/opencode-session-migration/spec.md`（经 delta 归档）。
- 测试：v2 schema fixture（`session_v2` + `project`）、v1 兼容 fixture、同库双表（v2 优先且 v1 行不被改动）、schema 缺失/目标歧义的拒绝路径；手工用真实 DB 的副本验证 13 条 session 的预览与迁移结果。
- 不改配置键、不改 jj 命令、不改删除对话框布局与交互，也不改变「迁移先于 forget/目录删除」的管线顺序。
