# Design: remove 前迁移 opencode session 到 jj 主仓库

## Context

插件 `remove` action 在 `jj workspace forget` 后 `fs::remove_dir_all` 删除工作目录。opencode（v1.18.30，源码快照 193de13a）的 session 与目录绑定：

- 存储于单一 SQLite `~/.local/share/opencode/opencode.db` 的 `session` 表（WAL 模式）；`directory` 列（绝对路径）即绑定，`project_id` 列决定项目归属。
- 可见性两层过滤：TUI picker → `listByProject`（`packages/opencode/src/session/session.ts:962-985`）**首先按 `project_id` 精确过滤**，`path` 过滤是条件性的（仅当查询带子目录时）。
- session 创建时 `projectID = Project.resolve(directory)`；纯 jj 二级 workspace 无 `.git` → resolve 兜底 `ID.global`（`packages/core/src/project.ts:110-122`）。

目录删除后 session 行残留为 `project_id='global'` + `directory=<已删路径>`，永久不可见。用户 DB 实证：8 个历史孤儿（`workspace-green-valley-f4bb` 1 个、`workspace-agent-agnostic-bootstrap-paths` 7 个）。

opencode 官方迁移能力调研结论（两道墙，均已在源码与真实环境验证）：

1. **无外部调用通道**：唯一入口 `POST /experimental/control-plane/move-session`；无 CLI move 命令；opencode 不写任何 lock/port 发现文件（端口默认 4096 冲突则随机，URL 仅打印 stdout，`server/server.ts:117-122`、`cli/cmd/serve.ts:20`）；TUI 内嵌 server 走虚拟 RPC。外部进程无从定位 pane 内 opencode 的 server。
2. **project mismatch（结构性）**：move-session 强制目标与 session 同 project（`core/src/control-plane/move-session.ts:83-87` `DestinationProjectMismatchError`）。jj 二级 workspace 即使主仓库 colocated 也无 `.git`（`jj workspace add` 无 colocate 选项），解析为 `global`，而 colocated 主仓库解析为 git id → 必然 400。已在用户真实 DB 实证两边的 project id。

## Goals / Non-Goals

**Goals:**

- `remove` 删除目录前，把绑定在 workspace（含子目录）下的全部 opencode session 迁移到主仓库，使其在主仓库 TUI 中可见、可续聊。
- 迁移语义与 opencode 官方行为逐列一致（`SessionEvent.Moved` 投影 + adoption 的 project_id 改写）。
- 对未安装 opencode / 无 session 的环境零影响（静默跳过）。
- 所有失败模式 fail-closed：数据无损、目录原样、可重试。

**Non-Goals:**

- 不迁移 uncommitted 变更（`check_remove_clean` 已保证工作副本干净）。
- 不修复历史孤儿 session（一次性手工 SQL，见 Migration Plan；插件只管自己经手的 workspace）。
- 不支持 opencode 旧版 JSON storage（v2 SQLite 之前的数据布局）。
- 不通过启动脚本/`.git` 伪装改变 session 创建时的 project 归属（已否决，见 Decisions D0）。
- 不触碰 `event`/`session_message`/`part` 表（只按 session id 键控，迁移不改 id，历史事件链天然完整——`core/src/database/migration/20260323234822_events.ts` 建表实证）。

## Decisions

### D0（否决项记录）：`.git/opencode` id 文件方案 — 已否决

在 workspace 内 `git init` + 写 `.git/opencode`（内容 = 主仓库 project id）可让 opencode 将 workspace 解析为主仓库 project。调研中已端到端验证可行（含官方 move-session API 返回 204、实例启动 adoption 自动改写既有 session）。**用户否决**：给 jj workspace 引入第二 VCS 状态对开发期工具链是长期不可靠变量。

特别记录：其变体「`.git` gitdir 指针文件指向主仓库」**绝对禁止**——实测中共享 git index 令 `git status` 报 363 个幻影删除，jj 快照将其中一个（`.codex/hooks.json`）误记为真实删除进工作副本提交（已用 `jj restore` 修复）。混合 VCS 状态的破坏路径是真实的，非理论风险。

### D1：直改 SQLite，而非官方 API 或 `opencode db` 写入

- 官方 API：两道墙（见 Context），不可用。
- `opencode db` 子命令：`db.all(sql.raw())` 只对 SELECT 有定义行为（`cli/cmd/db.ts:25-42`），依赖它执行 UPDATE 是未定义行为；仅用其 `db path` 子命令（`:45-52`）发现 DB 路径（覆盖 `OPENCODE_DB`/XDG/channel 变体）。
- 直改的正当性——**写法全部来自 opencode 自己的生产代码先例**：

| 先例 | 位置 | 行为 |
|---|---|---|
| `SessionEvent.Moved` 投影 | `core/src/session/projector.ts:242-256` | UPDATE `directory`/`path`/`workspace_id=NULL`/`time_updated`（`project_id` 不动——官方场景两者必同 project） |
| adoption | `opencode/src/project/project.ts:295-302` | 实例启动时批量 UPDATE `project_id='global'` 且 `directory=<本目录>` 的 session 为解析出的 project id——非事件溯源直写 |
| migrateProjectId | `project.ts:150-192` | project id 变更时事务内直写 `session.project_id` |

本方案 = Moved 投影列语义 + adoption 的 `project_id` 改写，两个官方行为拼接，无发明语义。

### D2：并发写参数 —— WAL + `BEGIN IMMEDIATE` + `busy_timeout=5000` + `foreign_keys=ON`

- 实验实证（用户真实热库，3 个活 opencode 进程 + 会话自身持续写入）：`BEGIN IMMEDIATE + UPDATE + ROLLBACK` 采样 30 次，`busy_timeout=5000` 下 **30/30 成功**（总 76ms）；默认 timeout=0 下 28/30（2 次 SQLITE_BUSY）。
- 必须显式设 busy_timeout（rusqlite 默认 0），必须 `BEGIN IMMEDIATE`——WAL 下延迟事务升级写锁返回的 `SQLITE_BUSY_SNAPSHOT` **不被 busy_timeout 重试**，立即开写事务可规避。
- 参数与 opencode 自身连接一致（`core/src/database/database.ts:27-32`：WAL/NORMAL/5000/foreign_keys=ON）；我们持锁仅数毫秒，反向阻塞无感。
- `foreign_keys=ON` 是第二道防线：非法 `project_id` 值被 SQLite 直接拒绝。

### D3：目标 project 解析链 —— 只用 opencode 自己算好的答案，绝不复刻哈希

`Project.resolve` 的优先级是 `remote-hash ?? <commonDir>/opencode 缓存 ?? root commit`（`project.ts:113-116`）；`commit()` 会把最终 id 写回 `<commonDir>/opencode`（`:118-120`）。因此缓存文件与 DB `project` 行就是 opencode 对该仓库的权威答案。

**主仓库根 `<main>` 的获取（链的先决步骤）**：复用插件既有 `repo_root()`（`src/main.rs:2210`）——二级 workspace 的 `.jj/repo` 是**文件**，内容为指向主仓库 store 的相对路径（相对 `.jj/`）；跟随指针、去掉尾部 `.jj/repo`、canonicalize 即得主仓库根。该函数异常时回退返回输入路径，迁移调用方 MUST 检测：解析结果与 workspace 自身相同即视为解析失败 → fail-closed 拒绝，绝不把 workspace 自身当主仓库。选它而非新增 `jj workspace list` 子进程解析：纯文件系统读取、已在 wizard/setup 路径长期使用（`main.rs:797/824/1591/1752`），行为一致。另注：`repo_root` 输出为 canonical 路径，与 DB `worktree` 值若有 symlink 差异由解析链兜底 fail-closed。

解析链（全部只读）：

```
(0) 主仓库无 .git（纯 jj）        → target = 'global'
    （此时 ws session 本就是 global，仅改 directory；global 行因 FK 必然已存在）
(1) 读 <main>/.git/opencode 缓存文件 → id；SELECT project 行 by id 确认存在 → 用
    （缓存文件是 resolve 的 memo，含 remote-hash 结果；repo 移动等场景下比按 worktree 查行更可靠）
(2) SELECT id FROM project WHERE worktree = <main> → 用
(3) 都不满足 → fail-closed 拒绝删除，toast 指引"先在主仓库打开一次 opencode 后重试"
```

**不复刻** `sha1("git-remote:" + 规范化URL)`：SSH/HTTPS/SCP 形态规范化只验证过一个用例，复刻是自造风险点。（曾为可选兜底分支，否决——fail-closed + 30 秒人工指引换取零复刻。）

顺带记录 shell 参考实现的两个保真度偏差（若保留 `projid()` 工具需修正）：`root(repo)` 取 `rev-list` 首行**不排序**（`| sort | head -1` 在多 root 仓库会偏离）；remote 只认名为 `origin` 的 remote。

### D4：枚举用范围比较，禁止 `LIKE`

session 可绑定在子目录（agent 在子目录工作），子 agent session 也各占一行（用户 DB 实证 7 个）。需匹配 `<ws>` 与 `<ws>/**`。**不用 `LIKE :ws || '/%'`**：`_` 是 LIKE 单字符通配符，而 herdr workspace 名大量含下划线。实测演示：

```sql
-- 迁移目标 workspace-test-proj_id（下划线）
'/x/workspace-test-proj-id/s' LIKE '/x/workspace-test-proj_id/%'  → 1  -- 误命中 sibling！
范围比较                                                            → 0  -- 正确
```

正确写法（`'/'` ASCII 0x2F 的后继是 `'0'` 0x30，`[ws+'/', ws+'0')` 恰好框住 `ws/**`）：

```sql
WHERE directory = :ws
   OR (directory >= :ws || '/' AND directory < :ws || '0')
```

纯比较必然走 B-tree 索引（`||` 是 SQL 字符串连接）。注意运算符优先级：比较表达式需括号包裹。

### D5：统一拍平到主仓库根（`path=''`）

官方语义：destination=根时 `subdirectory = path.relative(main, main) = ''`（`move-session.ts:106-110`）。不保留子目录结构的理由：

- `listByProject` 的 `path` 过滤是条件性的——根目录 picker（主入口）对 `path=''` 与 `path='sub'` 都可见，拍平无可见性损失；
- 保留 `path='sub'` 则续聊要求 `main/sub` 在磁盘上存在——jj workspace 常为 sparse checkout 或基于不同父版本，`ws/sub` 在主仓库工作副本中很可能不存在（用户真实 workspace sparse 列表仅 `.agents/.codex/AGENTS.md` 数项）；
- 拍平让"目录不存在怎么办"的问题整体消失。

### D6：`workspace_id = NULL` —— 三个 workspace 概念消歧

| 概念 | 本质 | 标识 |
|---|---|---|
| jj workspace | jj 二级工作副本（用户语境的 workspace） | 目录路径 |
| **opencode workspace** | opencode control-plane 的**托管执行环境**（内置 worktree adapter "Create a git worktree"；可注册远程 adapter，`Target` 可为 `{type:"remote", url}`）——opencode 版云沙箱 | `wrk_` 前缀 id（`schema/src/workspace-id.ts`） |
| opencode worktree | 上一行的内置实现机制 | `data/worktree/<project>/` |

`session.workspace_id` 指第二种。它维护的不变量是"**该 session 的执行上下文当前由哪个托管环境持有**"（warp 进环境→盖章；move 到裸目录→所有权解除→NULL，`projector.ts:250`）。move API 的 destination 只有 `{directory}` 形态，裸目录承载不了环境所有权，故 NULL 是不变量的推论而非善后。该列**无 FK**（`session/sql.ts:30` 纯 text 列，对比 `project_id` 有 `.references(..., cascade)`）；本地 TUI 列表不消费它（`session.ts:964-965` 仅 `?workspace=` 云路由查询才过滤）。用户 DB 实证：`workspace` 表 0 行、14 个 ws session 全为 NULL——此 SET 今日是 no-op，保留理由：逐列复刻官方投影（可 diff 性）+ 防御未来出现 wrk_ 环境（届时 NULL 依然正确）。

### D7：schema 探测 fail-closed

39 个历史迁移文件全量扫描：`session`/`project` 表演进**全部为 ADD COLUMN**（session 8 次：workspace_id/path/agent/model/cost/tokens_*/metadata），破坏性先例只在别的表（`workspace`/`session_input`/`project_directory` 整表重写、`session_context_epoch` 删列、2026-06 v2 重构期两次清空 `session_message`/`event`）。未来不可预测，故把预测换成探测：

- 执行前 `PRAGMA table_info(session)` 必需列 `{id, project_id, directory, path, workspace_id, time_updated}` 齐全、`PRAGMA table_info(project)` 含 `{id, worktree}`；任一缺失 → 拒绝删除（fail-closed），绝不盲写。
- 迁移器为独立模块；官方未来提供 CLI 或修复 project 语义时整体替换。

### D8：跳过与拒绝的边界

| 情形 | 行为 |
|---|---|
| PATH 无 `opencode` | **跳过**（非错误，继续删除）——插件的 agent 不可知哲学：清理不依赖 pane 里跑的是哪个 agent |
| `opencode db path` 失败 / DB 文件不存在 | **跳过**（无数据可迁移） |
| 枚举计数 = 0 | **跳过** |
| DB 打不开 / 枚举 SQL 失败 / schema 探测失败 / 目标 project 不可解析 | **拒绝删除**（fail-closed：stderr + toast，不执行 forget/rm/tab close，数据无损可重试） |

### D9：插入点 —— `check_remove_clean` 之后、`jj workspace forget` 之前

所有破坏性操作（forget/rm/tab close）排在迁移成功之后；任何失败点之前数据与目录完好，与既有 `check_remove_clean` 哲学一致。失败消息走既有 `die()` 通道（toast title≤80 / body≤240 限制）。

### D10：依赖与实现形态

- `rusqlite`（`bundled` feature）——自包含二进制，不依赖系统 sqlite3/dev 头文件。
- 迁移成功 toast：`migrated N opencode session(s) to <main>`（复用 `agent_attention_toast` 模式或 `die` 的正向变体）。
- 同事务顺带清理 `project_directory` 表中 `directory` 在 `<ws>/**` 范围内的行（实例启动时 `saveProjectDirectory` 写入的目录→project 映射；目录将不存在，残留行对同名新 workspace 是误导）。表不存在（旧 schema）则静默跳过该语句。

## Risks / Trade-offs

- [上游 schema 演进破坏写法] → D7 探测 + fail-closed + `foreign_keys=ON` 兜底；核心列历史只加不减；design 记录验证版本 1.18.30，升级 opencode 后需回归验证。
- [绕过事件流，运行中 TUI 不实时感知] → 目标 TUI（workspace 内的）随 tab close 即将退出；主仓库 TUI 每次 picker 查询直查 DB（`SessionStore.get` 无内存缓存，已核实）；代价与官方 adoption 直写同级。
- [与 adoption 竞态（迁移瞬间用户恰在主仓库打开 opencode）] → adoption 只收养 `project_id='global'` 行；我们已写入目标 id，语义收敛，双向幂等。
- [`project` 行缺失但缓存文件存在等罕见组合] → 一律 fail-closed 拒绝 + 指引，不做 INSERT 兜底（minimal INSERT 需复刻 NOT NULL 语义，收益低风险高）。
- [版本耦合 1.18.x] → 迁移器独立模块 + D7 探测；官方出 CLI 即替换。
- [rusqlite bundled 增大二进制] → 接受（插件本身以 opt-level=z/lto/strip 压缩）。

## Migration Plan

1. 实现 → 单测（D4 边界、D3 分支、D7 拒绝路径）→ fixture DB 集成测试 → 沙盒 e2e（隔离 `OPENCODE_DB` + `opencode serve --port 0` + ws 建 session + 跑 remove + 验证行与主仓库 TUI 可见性）。
2. **一次性存量修复**（独立于插件，不进代码）：对 8 个历史孤儿执行手工 SQL——
   ```sql
   UPDATE session SET project_id='<主仓库id>', directory='<主仓库>',
                      path='', workspace_id=NULL, time_updated=strftime('%s','now')*1000
   WHERE directory IN ('<两个已删目录>'） OR <同范围条件>;
   ```
3. 回滚：迁移本身无部署状态；插件回滚 = 旧版本二进制（旧版无迁移步骤，行为回到现状）。

## Open Questions

无阻塞性未决项。Watch items：上游若发布官方 session-move CLI 或修正非 git 检出的 project 语义 → 整体切换到官方通道（替换点已隔离在迁移模块）。

## Appendix: one-time orphan cleanup (concrete)

The 8 historical orphan sessions observed before this change (queried
2026-09-11, `opencode db path` → `~/.local/share/opencode/opencode.db`) are
**not** touched by the plugin — `remove` only migrates the workspaces it
removes from now on. This is the manual, out-of-band fix to run once (back up
the DB first; the write follows the same WAL + immediate-transaction
discipline as the migrator). It uses the same range predicate, never `LIKE`.

| Orphaned workspace directory | Sessions | Target project id | Target directory |
|---|---|---|---|
| `/home/cyc/Workspace/MultiSurveyImageEncoder/workspace-green-valley-f4bb` | 1 | `41c36e608331afe8b43cbd77a52e9dd3e3916ae2` | `/home/cyc/Sources/MultiSurveyImageEncoder` |
| `/home/cyc/Workspace/herdr-plugin-jj-workspace/workspace-agent-agnostic-bootstrap-paths` | 7 | `2d6936093cde97cf0ddc08e26270d0ccc7b01697` | `/home/cyc/Sources/ThirdParty/herdr-plugin-jj-workspace` |

```sql
-- Manual, out-of-band fix (NOT executed by the plugin).
-- Run both UPDATEs in one transaction; back up opencode.db first.
BEGIN IMMEDIATE;
UPDATE session
   SET project_id    = '41c36e608331afe8b43cbd77a52e9dd3e3916ae2',
       directory     = '/home/cyc/Sources/MultiSurveyImageEncoder',
       path          = '',
       workspace_id  = NULL,
       time_updated  = strftime('%s', 'now') * 1000
 WHERE directory = '/home/cyc/Workspace/MultiSurveyImageEncoder/workspace-green-valley-f4bb'
    OR (directory >= '/home/cyc/Workspace/MultiSurveyImageEncoder/workspace-green-valley-f4bb' || '/'
        AND directory <  '/home/cyc/Workspace/MultiSurveyImageEncoder/workspace-green-valley-f4bb' || '0');
UPDATE session
   SET project_id    = '2d6936093cde97cf0ddc08e26270d0ccc7b01697',
       directory     = '/home/cyc/Sources/ThirdParty/herdr-plugin-jj-workspace',
       path          = '',
       workspace_id  = NULL,
       time_updated  = strftime('%s', 'now') * 1000
 WHERE directory = '/home/cyc/Workspace/herdr-plugin-jj-workspace/workspace-agent-agnostic-bootstrap-paths'
    OR (directory >= '/home/cyc/Workspace/herdr-plugin-jj-workspace/workspace-agent-agnostic-bootstrap-paths' || '/'
        AND directory <  '/home/cyc/Workspace/herdr-plugin-jj-workspace/workspace-agent-agnostic-bootstrap-paths' || '0');
COMMIT;
```
