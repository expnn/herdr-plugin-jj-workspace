## 1. 依赖与骨架

- [x] 1.1 `Cargo.toml` 添加 `rusqlite`（`bundled` feature），`cargo build` 确认自包含编译通过
- [x] 1.2 新建迁移模块骨架（如 `src/opencode_migration.rs`）：错误枚举（Skip / Refuse / 迁移失败）与顶入口 `migrate_opencode_sessions(ws: &Path, main_repo: &Path)`，供 `cmd_remove` 调用

## 2. DB 发现与 schema 探测

- [x] 2.1 实现 DB 路径发现：子进程 `opencode db path`（PATH 无 opencode 或命令失败 → Skip；输出路径不存在 → Skip）
- [x] 2.2 实现 schema 探测：`PRAGMA table_info(session)` 必需列 {id, project_id, directory, path, workspace_id, time_updated}、`PRAGMA table_info(project)` 必需列 {id, worktree}；任一缺失 → Refuse（fail-closed，消息含原因）
- [x] 2.3 打开连接参数对齐 opencode：WAL、`busy_timeout=5000`、`foreign_keys=ON`；打开失败 → Refuse

## 3. 目标 project 解析链（D3）

- [x] 3.0 实现主仓库根解析：复用 `repo_root()`（`src/main.rs:2210`）；检测 fallback——解析失败或结果等于 workspace 自身 → Refuse（不拿 workspace 冒充主仓库）
- [x] 3.1 实现 (0) 主仓库无 `.git` → target = `global`
- [x] 3.2 实现 (1) 读 `<main>/.git/opencode` 缓存文件 + `project` 表行确认存在 → 用该 id
- [x] 3.3 实现 (2) `SELECT id FROM project WHERE worktree = :main` → 用该 id
- [x] 3.4 实现 (3) 均不满足 → Refuse，消息指引"先在主仓库打开一次 opencode 后重试"

## 4. 枚举与迁移事务（D1/D4/D5/D6）

- [x] 4.1 实现范围枚举：`directory = :ws OR (directory >= :ws || '/' AND directory < :ws || '0')`，计数为 0 → Skip（无提示）
- [x] 4.2 实现单事务迁移：`BEGIN IMMEDIATE` → 批量 UPDATE（`project_id=target`、`directory=main`、`path=''`、`workspace_id=NULL`、`time_updated=now_ms`）→ 同事务删除 `project_directory` 中 `directory` 在 `<ws>/**` 范围内的行（表不存在则静默跳过该语句）→ COMMIT；失败整体回滚 → Refuse
- [x] 4.3 迁移成功（N ≥ 1）toast：`migrated N opencode session(s) to <main>`（遵守 herdr toast title≤80 / body≤240 限制）

## 5. remove 流程接线（D9）

- [x] 5.1 `cmd_remove` 在 `check_remove_clean` 之后、`jj workspace forget` 之前解析主仓库根（`repo_root()`，失败即 Refuse）并将 workspace 与主仓库根传入迁移入口；Skip → 继续原流程；Refuse → 走 `die()` 通道拒绝（不执行 forget / 目录删除 / tab 关闭）
- [x] 5.2 复核 `die_toast_body` 对迁移拒绝消息的截断表现，确认指引文字完整可读

## 6. 测试

- [x] 6.1 单测——枚举边界：根命中 / 子目录命中 / 下划线相似名兄弟目录不命中（`workspace-test-proj_id` vs `workspace-test-proj-id`）/ 前缀相似但更长的目录不命中
- [x] 6.2 单测——解析链四分支：纯 jj → global；缓存文件命中；DB 行命中；全缺 → Refuse
- [x] 6.3 单测——探测拒绝路径：缺列 → Refuse；计数 0 → Skip；无 opencode → Skip
- [x] 6.4 集成测试——fixture DB（按 v1.18 schema 建表 + 种子 session 行）跑完整迁移器：验证列值、事务原子性（中途失败无部分写入）、`project_directory` 清理、外键拒绝非法 project_id
- [x] 6.5 沙盒 e2e 验收：隔离 `OPENCODE_DB` + `opencode serve --port 0` → 在沙盒 ws 建 session（含子目录）→ 跑 `remove` → DB 行验证（project_id/directory/path/workspace_id）→ 主仓库 session list（TUI picker 同一查询路径）确认可见 + `opencode run --session <migrated>` 确认可续聊
- [x] 6.6 回归：无 opencode 环境跑 `remove` 全流程照常；脏工作副本拒绝消息不受迁移步骤影响（顺序：clean 检查仍最先）
- [x] 6.7 单测——主仓库根解析：正常 `.jj/repo` 指针解析成功；指针缺失/不可读/canonicalize 失败/结果等于 workspace 自身 → Refuse

## 7. 文档与收尾

- [x] 7.1 README "remove" 小节补一段：删除前自动迁移 opencode session 到主仓库，及 fail-closed 行为说明
- [x] 7.2 记录一次性存量修复 SQL（8 个历史孤儿 session，独立于插件代码，写进 change 的收尾说明或 docs）
- [x] 7.3 `cargo test` 101 passed / `cargo fmt --check` clean / `cargo clippy` 无新增警告（存量 8 条 pre-existing，不属本 change）；已按仓库惯例提交
