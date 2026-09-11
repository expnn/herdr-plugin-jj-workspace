## Why

插件删除 jj workspace 时会连同工作目录一起移除，而 opencode 的 session 与目录绑定（`~/.local/share/opencode/opencode.db` 的 `session.directory` 列）。目录消失后这些 session 行永久残留在 `project_id='global'` 名下，任何项目的 TUI 列表都看不到——历史数据已实证（用户 DB 中存在 8 个此类不可见孤儿 session）。删除前把 session 迁移回 jj 主仓库，可让对话历史在主仓库中继续可用。

## What Changes

- `remove` action 在 `jj workspace forget` 之前新增清理步骤：将 `directory` 绑定在待删 workspace（含其子目录）下的所有 opencode session 迁移到 jj 主仓库目录（UPDATE `session` 表的 `project_id`/`directory`/`path`/`workspace_id`/`time_updated`，语义对齐官方 `SessionEvent.Moved` 投影 + adoption 行为）。
- 迁移器带有运行时 schema 探测（必需列齐全才执行）与目标 project 解析链，任一失败即 fail-closed 拒绝删除（数据无损、可重试），与 `check_remove_clean` 同一哲学。
- opencode 未安装或无匹配 session 时静默跳过（非错误），保持插件对 agent 的既有不可知性。
- 迁移成功以 toast 上报迁移数量。

## Capabilities

### New Capabilities

- `opencode-session-migration`: remove 前将 workspace 绑定的 opencode session 迁移到主仓库——枚举语义、目标 project 解析链、直改 SQLite 的写法与并发/ schema 防护、fail-closed 边界与跳过条件。

### Modified Capabilities

- `workspace-removal`: remove 流程在 forget 之前插入迁移步骤；迁移失败时整个 remove 被拒绝（不执行 forget / 目录删除 / tab 关闭）。

## Impact

- **代码**：`src/main.rs` 新增迁移模块（DB 路径发现、schema 探测、project 解析、范围枚举、单事务 UPDATE、toast 上报）；`cmd_remove` 在 `check_remove_clean` 之后、`jj workspace forget` 之前插入调用。
- **依赖**：`Cargo.toml` 新增 `rusqlite`（`bundled` feature，避免依赖系统 sqlite3）。
- **外部系统**：opencode ≥1.18 的 `opencode.db`（WAL 模式，与运行中的 opencode 进程并发读写已实证安全）。
- **不受影响**：event/session_message/part 表（只按 session id 键控，迁移不改 session id，历史事件链完整）；`opencode db` CLI 仅用于读取路径，不用于写入。
