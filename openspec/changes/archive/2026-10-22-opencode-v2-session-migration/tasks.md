## 1. 活跃表选择与 schema 探测

- [x] 1.1 在 `src/opencode_migration.rs` 增加表存在性探测（`sqlite_master`）与活跃表选择：存在 `session_v2` → v2，否则 `session`；表名只能取内部常量，不拼接外部输入。单测覆盖仅 v1、仅 v2、双表、两表皆无四种情况
- [x] 1.2 把 `probe_schema` 拆为按活跃表探测：v2 检查 `session_v2` 必需列 {id, project_id, directory, path, workspace_id, time_updated}，v1 检查 `session` 同列，`project` 仍需 {id, worktree}；两表皆无 → 特定拒绝原因。单测覆盖 v2 缺列、v1 缺列、两表皆无

## 2. 枚举、计数与更新

- [x] 2.1 `count_sessions`、`select_sessions`、`count_selected_sessions`、`run_migration_tx` 的 SQL 改为操作所选活跃表；v2 下 `session` 表零写入。用 v2 fixture 单测覆盖：根 + 子目录、名称相近的兄弟目录不误命中、子 agent 行（parent_id 非空）、仅更新勾选 id、`project_id`/`directory`/`path=''`/`workspace_id=NULL`/`time_updated` 的精确值
- [x] 2.2 预览与执行共用同一活跃表选择逻辑；空选择 skip、预览/执行行集漂移按实际更新行数上报。补充对应单测与断言

## 3. 目标 project 解析

- [x] 3.1 `resolve_target` 按活跃表版本分派：v2 精确匹配 `worktree = canonical(主仓库根)` 且恰好一行；0 行或多行拒绝并给出「先在主仓库打开一次 opencode」/歧义说明；v1 保持既有链（纯 jj → global、`.git/opencode` 缓存、按 worktree 查询）。单测覆盖 v2 命中、v2 零行、v2 多行、v1 三条分支

## 4. 清理与跳过/拒绝边界

- [x] 4.1 v1 路径保留 `project_directory` 范围删除；v2 路径不删除任何 `project` / `worktree` 行。单测断言迁移前后 v2 的 project/worktree 行数不变
- [x] 4.2 收紧边界：发现缺失与枚举计数 0 仍为 Skipped；v2 表存在但缺列、两表皆无、目标 project 歧义均为 Refused（对话框 ✗ 阻断），断言拒绝原因的关键文案

## 5. 验证

- [x] 5.1 `cargo test` 全绿；`openspec validate opencode-v2-session-migration --strict` 通过
- [x] 5.2 真实 DB 副本验证：复制 `~/.local/share/opencode/opencode.db*` 到临时目录，以 `XDG_DATA_HOME` 指向副本，运行只读预览确认列出该 workspace 的 13 条 session（按 id/title 核对），再对副本执行迁移并核对：13 行 `project_id` 变为 `827616d7…`、`directory` 变为主仓库根、`path=''`、`workspace_id` 为 NULL、`time_updated` 前进；`session` 表零变化
- [x] 5.3 真实删除端到端（需用户确认）：在 `/home/cyc/Workspace/WaveletsGen/workspace-upgrade-frontend` 上打开删除对话框，确认预览列出 13 条并可勾选；执行后确认 `jj workspace list` 不再包含该 workspace、目录被删除、真实 DB 中这 13 行已落到主仓库 project
