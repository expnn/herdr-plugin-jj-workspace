## Context

现状见 `proposal.md`「Why」。与本设计相关的既有实现与被上游改变的假设：

- `src/opencode_migration.rs` 的预览与执行共用同一条链：`inspect_opencode_sessions` / `migrate_selected_opencode_sessions` → `inspect_db` / `migrate_selected_db`，其中 `count_sessions`、`select_sessions`、`count_selected_sessions`、`run_migration_tx` 的 SQL 固定 `FROM session`；`probe_schema` 固定探测 `session` 列；`resolve_target` 固定 v1 解析链；写入后删除范围内 `project_directory` 行。
- 本机实测（2026-10-21，opencode 2.0.29，DB `~/.local/share/opencode/opencode.db`）：`session` 596 行，version 全为 `1.18.x`，最后 `time_updated` 为 2026-10-16 10:46:59；`session_v2` 925 行，其中 331 行为 2.x 独有（`2.0.18`～`2.0.29`）；待删 workspace `/home/cyc/Workspace/WaveletsGen/workspace-upgrade-frontend` 的 13 条 session 全部在 `session_v2`（project `8a903beb…`，其 `worktree` 即该 workspace），`session` 表中为 0 条。
- 上游一手依据（本机 `~/Sources/ThirdParty/opencode`，v2.0.30 checkout）：
  - `packages/core/src/session/sql.ts:23`：`SessionTable` 绑定 `"session_v2"`——v2 的会话表。
  - `packages/core/src/database/v1-migration.bun.ts:611,788`：v2 升级迁移把 v1 `session` 行写入 `session_v2`（`INSERT OR IGNORE` / `INSERT`），因此升级后的 DB 中 `session_v2` 是历史行 + v2 新行的唯一活跃存储。
  - `packages/core/src/session/projector.ts` 的 `SessionEvent.Moved` 投影精确更新 `directory`、`path`、`project_id`、`workspace_id`、`time_updated`——迁移的列语义依据。
  - `packages/core/src/project.ts:392`：非 git/hg 目录（如无 `.git` 的 jj 副 workspace）以 `Hash.fast("directory:…")` 生成 project id；v1 的「纯 jj → global」在 v2 不成立。
  - `packages/core/src/project.ts` 的 git 分支仍读 `commonDirectory/opencode` 缓存，但 remote 优先，缓存可能过期；`project.worktree` 是 opencode 自己落盘的权威映射。
  - `packages/core/src/project.ts` 的 `sweep` 会把目录已消失的 project 归档（或切到存活的 main checkout），插件无需删除 project 行。
- 既有约束：迁移必须发生在 `jj workspace forget` 与目录删除之前；DB 可能与运行中的 opencode 并发写；v1 用户与既有 spec 语义必须保持不变。

## Goals / Non-Goals

**Goals:**

- 在 v2 DB 上正确检测、预览并迁移 `session_v2` 中绑定待删 workspace 的 session，列语义与 opencode 官方 Moved 投影逐列一致。
- 版本分派只依据 schema 存在性（`session_v2` 是否存在），不解析版本字符串。
- 未知/不兼容 schema（含未来变化）fail-closed，不再以「0 匹配」静默跳过。
- v1-only DB 的行为与既有实现逐列一致。

**Non-Goals:**

- 不修复 opencode v2 移除的 `opencode db path` 子命令；DB 发现已有标准路径回退，本机实测可命中（若未来发现链失效，单独 change）。
- 不删除或改写 `project`、`worktree`、v2 的 `project_directory` 行，不写 opencode `event` 日志。
- 不改变删除对话框布局、勾选交互、Status 阶段顺序与管线顺序。
- 不提供 v1↔v2 双向同步或降级支持；v2 下 v1 历史表保持原样。

## Decisions

### 决策 1：活跃表按 schema 存在性选择，且只操作所选表

存在 `session_v2` 表时以它作为唯一枚举与更新目标；否则使用 `session`。表名只来自内部常量，不拼接外部输入。

理由：v2 升级把 v1 行复制进 `session_v2`，且 v2 只写 `session_v2`；同一 id 可能同时存在于两表，双表更新会重复计数且对 v2 读取路径无收益，双表「取并集」还会让 v1 历史行与 v2 活跃行混在一起。

备选（未采纳）：按 `opencode --version` 分派——版本字符串与 schema 状态可能不一致（升级中断、迁移未完成）；两表都更新——增加写入面且不改变 v2 行为。

### 决策 2：列语义严格对齐官方 Moved 投影

更新列固定为 `project_id = 目标`、`directory = canonical(主仓库根)`、`path = ''`、`workspace_id = NULL`、`time_updated = 当前毫秒`。

理由：`session/projector.ts` 的 `SessionEvent.Moved` handler 即 opencode 对「session 移动到另一目录」的权威定义；`path` 是相对 project 目录的 subpath，目标为主仓库根时为空串（实测主仓库 project 的 97 条 v2 行全部 `path=''`，`workspace_id` 全部为 NULL）。

备选（未采纳）：调用 opencode 内部 `SessionMove` 服务——v2 没有稳定的外部 CLI/接口，且需要运行中的 server 与 Location 上下文；沿用既有「直接 SQLite + 单事务 + 与 opencode 相同的并发参数」方案。

### 决策 3：v2 目标 project 按 `project.worktree` 精确匹配，0/多行都拒绝

在 `session_v2` 活跃时，以 `worktree = canonical(主仓库根)` 查询 `project`：恰好一行才使用其 `id`；0 行或多行均拒绝删除并给出可操作指引。

理由：v2 对非 git 目录使用 `directory:<hash>` id（`project.ts:392`），v1 的 `global` 假定不再成立；`project.worktree` 是 opencode 自身持久化的答案，无需复刻哈希（延续 spec 的 MUST NOT）。`.git/opencode` 缓存虽然仍被 v2 读取，但 remote 优先、缓存可能过期，不能作为离线迁移的权威来源。

备选（未采纳）：v2 也先读 `.git/opencode`——可能指向 remote 变更前的旧 project id；用 `global` 兜底——与 v2 语义不符。

### 决策 4：探测对象随活跃表变化；不兼容即拒绝

按所选活跃表探测必需列 {id, project_id, directory, path, workspace_id, time_updated}，`project` 仍需 {id, worktree}；两表都不存在、活跃表缺列、目标 project 歧义 → 一律拒绝删除。静默跳过仅保留：发现缺失（无二进制且标准位置无 DB）与枚举计数为 0（含未勾选）。

理由：本次缺陷正是「表还在、列还在，但数据已迁走」造成的静默 skip；把「活跃表存在但不可执行」提升为拒绝，可防止未来 opencode 再换存储时重演。

### 决策 5：v2 不做 project/worktree 清理；v1 保留 `project_directory` 清理

理由：`Project.sweep` 会归档目录已消失的 project；`worktree` 行由 opencode 的 worktree 服务管理，jj workspace 不是其管理的 git worktree（实测 0 行）。删除 `project` 行还会触发 `session_v2.project_id` 的 `ON DELETE CASCADE`，存在误删 session 的风险。

备选（未采纳）：迁移后删除 workspace 的 project 行——无必要且危险。

### 决策 6：事务、并发与子集语义不变

复用 `open_and_probe`（WAL 现状、`busy_timeout=5000`、`PRAGMA foreign_keys=ON`）、`BEGIN IMMEDIATE` 单事务、勾选子集、预览/执行漂移按实际更新行数上报。

## Risks / Trade-offs

- [运行中的 opencode 可能持有迁移前的内存状态] → 与既有 v1 设计边界一致：迁移在 forget 前完成，对话框展示目标；删除期间避免继续向该 workspace 发消息。
- [v2 纯 jj 主仓库从未被 opencode 打开过 → 无 `project.worktree` 行] → fail-closed 拒绝并指引「先在主仓库打开一次 opencode」；不猜测 id。
- [`project` 表存在同 worktree 的历史重复行] → 拒绝并提示歧义，不擅自选择。
- [未来 opencode 再改表名/列] → schema 探测 fail-closed；spec 明确「存在 `session_v2` 即以其为唯一目标」，改动前必须先更新本插件。
- [v2 下 v1 历史表不更新，若用户回退 v1 会看到旧目录] → 明确非目标（v2 数据库不保证可被 v1 读取）；v1-only DB 仍按原语义工作。
- [测试 fixture 与真实 schema 漂移] → fixture 以本机 v2 schema 为蓝本，并增加对真实 DB 副本的迁移验证任务。

## Migration Plan

- 纯插件内部行为变化，无配置或数据迁移；发布后首次删除 v2 workspace 即生效。
- 回滚：回退二进制即恢复 v1-only 行为（v2 再次静默跳过，不产生数据损坏）；已迁移的 `session_v2` 行不会因回滚失效。

## Open Questions

（无）
