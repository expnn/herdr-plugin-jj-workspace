# Design: remove-workspace-dialog

## Context

`remove` 现状（`src/main.rs` `cmd_remove`）：headless 单步执行——读 `HERDR_PLUGIN_CONTEXT_JSON.workspace_cwd`（即聚焦 pane 的 shell cwd），要求该目录恰好含 `.jj` 且为副 workspace；先 `check_remove_clean`，再迁移 opencode session，然后 `jj workspace forget`、`rm -rf`、`herdr tab close <tab_id>`。问题（proposal）：无审阅、关 tab 过激、目标解析过窄（子目录误报）、主 workspace 与 stale 注册不可达。

本 change 的形态与创建 wizard 对称：action 只预检，交互与执行都在 overlay plugin pane 中进行。以下决策基于已求证的平台行为（标注来源）：

- **herdr 0.8.2 实测**（本机运行中的 server；0.9.0 CLI 因协议 22 vs 20 无法直连）：plugin overlay pane 打开后出现在 `herdr pane list`（`cwd` = plugin_root、`label` = `[[panes]]` title、`focused` = true）；进程退出自动关闭且可稳定驻留交互；`pane close` 可关闭它；`pane process-info` 对 overlay pane 返回 `pane_not_found`；`plugin pane close` 对 overlay pane 报 `plugin_pane_not_found`（错误不可靠）。（2026-09-14 下午本机 server 升级到 0.9.0；最终验收冒烟 S1–S8 在 0.9.0 上完成，overlay / `pane list` / `pane close` 行为一致。）
- **herdr 源码**（`src/workspace.rs:close_pane`、`src/app/api/panes.rs`）：关闭 tab 内最后一个 pane → tab 关闭；关闭 workspace 内最后一个 tab → workspace 关闭；worktree group 场景存在 `confirmation_required` 保护。
- **jj 0.45.1 实测**：`jj -R <main> --ignore-working-copy workspace list -T 'name ++ "\t" ++ root ++ "\n"'` 输出 `name<TAB>绝对路径`（与 cwd 无关）；目录被删的 workspace `root` 渲染为空；`jj workspace forget <name>` 无目录也能成功。`.jj/repo/workspaces/` 在 0.45.1 已不存在（内部布局随版本变化），故不读内部文件。
- **opencode DB 实测**：`session` 表含 `title`(NOT NULL)、`directory`、`time_updated` 列，可直接支撑预览列表。
- **创建 wizard 先例**：action 预检 + `plugin pane open --entrypoint wizard` + 自身 context 重新解析（`wizard-single-source` 已归档），对话框可复用其 palette / section / 按钮 / hint / 错误行实现。

## Goals / Non-Goals

**Goals:**

- 删除前可审阅：Plan 任务清单 + 每个任务的对象（session / pane）可勾选，默认全选。
- 按 pane 关闭替代 tab 关闭；同 tab 的异目录 pane 与 tab 本身可存活。
- 主 workspace → 副 workspace picker（含 stale 注册清理）。
- 目标解析上溯子目录；对话框内指引 + 授权 commit 修复脏检查。
- 执行顺序即安全边界（复检 → 迁移 → forget → 删目录 → 关 pane），fail-closed 语义保持。
- 与创建 wizard 同族的视觉与交互语言。

**Non-Goals:**

- 不批量删除多个 workspace（picker 一次选一个）。
- 不代执行 `jj restore`（丢弃性操作仅展示）。
- 不保留 headless 快速旁路（不引入 `--yes`）。
- 不改 opencode 迁移的 fail-closed 探测链与事务语义（仅增加只读预览与子集参数）。
- 不在副 workspace 内提供 picker 入口（需回主仓库触发；后续可加）。
- 不新增配置键；不做版本号调整（是否 bump 留给发布流程）。

## Decisions

### D1 对话框以 overlay plugin pane 承载，action 只预检

`remove` action 保留为唯一入口（contexts=["workspace"] 不变），但只做：config / `jj.command` 预解析、目标上溯与守卫判定（非 jj / 不安全 → toast），随后 `herdr plugin pane open --plugin expnn.jj-workspace --entrypoint remove-wizard --focus`，行为对齐 `cmd_open`。manifest 新增 `[[panes]] id="remove-wizard"`（placement overlay，title "Remove jj workspace"）。对话框进程用自身注入的 context 重新解析目标（与 wizard spec 同原则，不经 `--env` 转发）。

备选：在 action 进程内渲染 TUI——action 无 TTY（stderr 无可见出口），否决；用 herdr 原生弹窗——herdr 无插件可用的原生 dialog API，否决。

### D2 目标解析 = focused_pane_cwd 上溯到 jj workspace 根

现状要求 cwd 恰为 workspace 根（`canon.join(".jj").exists()`），子目录触发误报 "not a jj workspace"。改为逐级上溯到最近含 `.jj` 的祖先（副 workspace 根），与 wizard 的 `jj_root()` 同一定义；**不**经 `repo_root()` 消解到主仓库根（要删的就是副 workspace 本身）。主 workspace 不再被拒绝：进入 picker（D3）。非 jj / 路径不存在 / 不安全路径（`/`、无父级）在 action 侧 toast。

### D3 主 workspace → 副 workspace picker（讨论决策点 D9，用户确认采纳）

数据源：`jj -R <main_root> --ignore-working-copy workspace list -T 'name ++ "\t" ++ root ++ "\n"'`。理由：`root` 是官方模板 API（`WorkspaceRef.root() -> Option<FsPath>`，0.38+ 记录路径），输出绝对路径且与 cwd 无关；目录已删时渲染为空——stale 自动可辨；`forget` 对 stale 注册也成功（实测），从而是唯一能清理 stale 的途径。

备选与否决：解析默认文本输出（路径相对 cwd、含 commit 摘要，格式不稳）；读 `.jj/repo/workspaces/<name>/working_copy`（0.45.1 已无该布局，内部格式）；`json(self)` 模板（不含 root）。pre-0.38 且未记录路径、以及目录已删除的 workspace（`root` 为空）：工作目录未知，降级为「仅 forget」（session 迁移、目录删除与 pane 关闭均无法定位对象，跳过），UI 与 Status 明示。选择器一次选一个（Non-Goal：批量）。

### D4 pane 候选与关闭（替代 tab 关闭）

候选 = `herdr pane list` 全局结果中 `cwd` **或** `foreground_cwd` 位于目标目录内（canonicalize + 路径边界比较，避免 `/a/b` 误配 `/a/bc`）的 pane。展示按 `herdr workspace list` / `tab list` 的 label 分层（workspace → tab → pane），行内含 pane id、agent/状态、相对 cwd、`^fg`（仅前台 cwd 命中）与 `(triggered here)` 标记。执行时逐个 `herdr pane close <id>`；不使用 `herdr tab close`。tab/workspace 关闭由 herdr 级联（Context 源码结论）。单一 pane 关闭失败（如 worktree group 的 `confirmation_required`）仅警告。

### D5 对话框自身 pane 的排除（不用 PID）

overlay pane 会出现在 `pane list`（实测），需从候选中排除自身。0.8.2 的 `pane process-info` 不支持 overlay pane（实测返 `pane_not_found`），PID 自识别方案不可用；改为规则排除：`cwd == plugin_root`（canonicalize）且 `label ∈ {本插件 pane 标题}`（"Remove jj workspace" / "New jj workspace"）。自身关闭不显式执行——进程退出即自动关闭（实测）；关闭 panke 阶段排在破坏性步骤之后，即使排除规则意外失误（如 plugin root 恰在目标目录内的开发链接场景）也不会损失已完成的删除。

### D6 session 预览与子集迁移（讨论决策点 D10，用户确认采纳）

`opencode_migration` 拆出只读 `inspect`：复用 find / discover / `probe_schema` / 同一范围谓词，返回绑定行 `{id, title, directory, time_updated}`、主仓库根或拒绝原因；不写 DB。预览失败按拒绝呈现为阻断检查（执行期仍独立 fail-closed，不因预览成功而放松）。迁移接受选中 id 子集；未勾选行 MUST NOT 更新；全未勾选按跳过。行集漂移按实际更新数上报。展示用 title（截断）+ 相对目录 + 时间；相对目录以目标目录为基准（`.` 表示根）。

备选：仅显示计数（用户已确认要列表+勾选）；在对话框内打开/管理 session（超出范围）。

### D7 修复执行边界：只代执行非破坏性、保工作的动作

脏工作副本时 Checks 逐字展示两条出路命令；`c` 子态提供单行 message 输入并执行 `jj commit -m <message>`（cwd = 目标目录、argv 直传、已解析 `jj.command`），成功后刷新检查。`jj restore` 仅展示、永不代执行。迁移类阻断（DB/schema/project）不可自动修复，仅展示原因与建议。

### D8 执行管线的阶段门控

`↵` → 复检 clean（TOCTOU 收敛；失败回审阅）→ 迁移（勾选子集）→ `jj workspace forget` → 删除目录（不存在时跳过）→ 关闭勾选 pane（best-effort）→ 进程退出。破坏性步骤失败即停：迁移失败不 forget；forget 失败不删目录；删除失败不关 pane；每步在 Status 视图反映；失败附 `error.log` 指针。成功无 toast 依赖。

### D9 渲染、尺寸与键位

复用 wizard 的 palette / `render_modal_shell` / section 标题 / 按钮 / 错误行实现；宽度 96；高度自适应 `clamp(内容高度, 31, min(终端高-4, 45))`，超出滚动（Plan 为主体），hint 与按钮固定。步骤序号渲染为 `1.`–`4.`；所有全路径展示统一将 `$HOME` 前缀缩写为 `~`（验收反馈）。鼠标：对话框启用 mouse reporting（herdr 侧据此走 `WheelRouting::MouseReport`，见 herdr `pane/terminal.rs`；未启用时 herdr 把滚轮编码成 ↑/↓——即验收观察到的“滚轮切换选项”），滚轮滚动内容坐标并显示滚动条，键盘语义不变。选择计数：步骤 1/4 的列表上方各有一行 dim 文本 `N of M selected`，随勾选实时更新（动态文本，无叶子时隐藏）；溢出滚动条在 review 与 picker 列表均渲染。键位：picker `↑↓ / ↵ / esc`；review `↑↓`、`space`（叶子切换、组头级联）、`a`（全局全选/全不选）、`c`（脏时）、`↵`、`esc`。0 个 pane 选中允许执行并保留警示（用户已确认）。

### D10 执行视图命名 Status（用户反馈 #7）

`↵` 后内容区切换为 `Status` section：任务逐项 `✓ / ◌ / · / ✗`；成功自动退出；失败停留 + `↵ close`。

### D11 版本号

`herdr-plugin.toml` / `Cargo.toml` 保持 0.5.0；本 change 不含 version bump（当前仓库无 tag，发布流程自行决定）。

### D12 DB 定位不依赖进程 PATH（验收回归修复）

问题（实测）：herdr server 以最小 PATH 启动（`/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:…`，不含 `~/.opencode/bin`），plugin pane 继承该环境 → `find_opencode` 返回 None，预览显示 “opencode not found on PATH”，session 列表无法呈现——尽管 DB 存在于标准位置（本机 `~/.local/share/opencode/opencode.db`）。范围澄清：预览本就只读 DB（SQL）；CLI 仅用于定位 DB 文件。

决策：发现链 = CLI 优先 + 标准位置回退。① `find_opencode` 除 PATH 外追加常见安装目录（`~/.opencode/bin`、`~/.local/bin`、`/usr/local/bin`）；② 可执行文件不可用，或 `opencode db path` 失败/输出无效时，回退 `$XDG_DATA_HOME/opencode/opencode.db`（默认 `~/.local/share/opencode/opencode.db`）；③ 预览与执行共用同一发现链（预览仍为只读 SELECT，CLI 从不写入）；④ 跳过原因必须包含已尝试的位置，避免不可诊断的 “not found on PATH”。

## Risks / Trade-offs

- [pane list 字段跨版本差异] → 解析取并用字段、缺失/异常条目跳过而非 panic；最终真实冒烟（S1–S8）在升级后的 0.9.0 server 上完成（渲染与 E2E、minimal-PATH、`~`+截断、滚轮+滚动条+计数）。
- [`.jj/repo/workspaces/` 内部布局不存在于 0.45.1] → 已改用官方模板 API；更老 jj（<0.38）root 未记录 → 降级仅 forget，UI 明示。
- [overlay pane 在 pane list 可见且 plugin root 可能位于目标目录内（开发链接场景）] → D5 规则排除 + 关闭阶段后置（破坏性步骤已完成）。
- [关闭 pane 与对话框自身生命周期的竞态（宿主 tab 被关时 overlay 消失）] → 破坏性步骤先于关 pane；自身不显式关闭；剩余 pane 关闭失败仅影响便利性。
- [列表交互复杂度（sessions + 组层级 panes）] → 选择模型做成纯函数状态机并单测覆盖（勾选/级联/计数/0 选中警示）。
- [herdr 对 worktree group 的 `confirmation_required` 保护拒绝个别 pane close] → 单项警告，不阻断流程。
- [真实冒烟依赖运行中的 0.8.2 server 且不可重启] → 单测（fake jj / fake herdr）+ 临时 jj 仓与 plugin 本地链接做端到端；不触碰用户的既有 workspace，测试目标一律在 `/tmp/opencode` 下新建。

## Migration Plan

无数据迁移（纯行为替换：`remove` 从单步执行改为对话框授权）。回滚 = git revert。发布时 README 删除章节与 Troubleshooting 同步更新，便于存量用户理解交互变化（BREAKING 标注）。

## Open Questions

（无——交互范围与两项新增决策（picker、session 勾选）已由用户确认；其余为实现细节。）
