# plugin-config Specification

## Purpose
TBD - created by archiving change config-foundation. Update Purpose after archive.
## Requirements
### Requirement: 配置文件唯一定位于插件配置目录
插件 SHALL 仅从 `$HERDR_PLUGIN_CONFIG_DIR/config.toml` 读取用户配置（路径由 herdr 注入的环境变量解析，MUST NOT 硬编码绝对路径）。配置文件不存在时，插件 SHALL 以全部内置默认值运行。插件 MUST NOT 将进程环境变量或 `.env` 文件作为配置来源。

#### Scenario: 配置文件不存在时使用默认值
- **WHEN** `$HERDR_PLUGIN_CONFIG_DIR/config.toml` 不存在
- **THEN** 插件以全部内置默认值正常运行（`jj.base_rev = "trunk()"`、`jj.workspace_root = "~/.herdr/workspaces"`、`agent.command = "codex"`、`agent.bootstrap_paths = ["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]`）

#### Scenario: 进程环境变量不再作为配置通道
- **WHEN** 进程环境中存在 `JJ_BASE_REV=main@origin`，且 config.toml 未定义 `jj.base_rev`
- **THEN** 插件使用内置默认值 `trunk()`，环境变量被忽略

### Requirement: 解析失败时 fail-fast
config.toml 存在语法错误、类型错误或空字符串值时，插件 MUST 拒绝运行并以明确错误退出，MUST NOT 静默降级到内置默认值。wizard 入口 SHALL 在界面中渲染错误信息；headless action（open/remove）SHALL 向 stderr 输出错误、以非零码退出、并自发 `herdr notification show` toast 呈现错误（herdr 不代为呈现 action 失败，stderr 无可见出口）。toast 受平台限制（≤240 字符单行、不可复制、~3s 消失），因此 SHALL 仅承载消息首行摘要（~120 字符）+ 完整日志路径指引；完整错误消息 SHALL 带 UTC 时间戳追加到 `$HERDR_PLUGIN_STATE_DIR/error.log`（写入失败时 best-effort 回退为仅摘要，不遮蔽原错误）。

#### Scenario: 语法错误阻断运行
- **WHEN** config.toml 内容为非法 TOML（如截断文件、缺少引号）
- **THEN** 调用任何插件 action 均以非零码退出并弹 toast 显示解析错误（`new` action 在 wizard 面板打开前即失败，错误经 toast 可见）；wizard 面板已打开时在界面中显示该错误

#### Scenario: 类型错误阻断运行
- **WHEN** config.toml 中 `jj.base_rev = 123`（数字而非字符串）
- **THEN** 插件拒绝运行，错误信息指明键名与期望类型

#### Scenario: 空字符串值视为无效
- **WHEN** config.toml 中 `agent.command = ""`
- **THEN** 插件拒绝运行，错误信息指明该键不允许空值（不再沿用"空串即默认值"的旧语义）

### Requirement: 未知键硬拒绝
配置解析 MUST 拒绝包含未知键的配置文件（`deny_unknown_fields` 语义），错误信息 SHALL 指出未知键名。

#### Scenario: 手误键名立即暴露
- **WHEN** config.toml 中定义了 `[jj] base_rev = "trunk()"` 与 `[agent] start_command = "codex"`（`start_command` 为不存在的旧名）
- **THEN** 插件拒绝运行，错误信息指出未知键 `agent.start_command`

### Requirement: 分节 schema 与内置默认值
配置文件 SHALL 使用分节结构：`[jj]` 节含 `base_rev`（字符串，revset 语法）与 `workspace_root`（字符串，支持 `~` 展开）；`[agent]` 节含 `command`（字符串）与 `bootstrap_paths`（字符串数组）。各键内置默认值 MUST 与迁移前行为一致：`base_rev = "trunk()"`、`workspace_root = "~/.herdr/workspaces"`、`command = "codex"`、`bootstrap_paths = ["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]`。

#### Scenario: 显式配置覆盖默认值
- **WHEN** config.toml 定义 `[jj] base_rev = "dev"` 与 `[agent] command = "opencode"`
- **THEN** 新工作区基于 revset `dev` 创建；左 pane 以 `opencode` 启动 agent

#### Scenario: workspace_root 支持 ~ 展开
- **WHEN** config.toml 定义 `jj.workspace_root = "~/code/workspaces"`
- **THEN** 新工作区检出目录解析到用户 home 下的 `~/code/workspaces/...`

### Requirement: bootstrap_paths 为跨 agent 共用的单一列表
`agent.bootstrap_paths` SHALL 为单一字符串数组，供所有 agent 类型共用：创建 jj 工作区时以 `jj sparse set --clear --add <path>...` 逐条传入。数组中不存在于仓库的路径 MUST NOT 导致错误或警告（由 jj sparse 机制静默跳过）。

#### Scenario: 引导文件物化
- **WHEN** 用户通过 wizard 创建 jj 工作区
- **THEN** 新工作副本的 sparse 模式被设为仅含 `bootstrap_paths` 列出的路径，对应存在的文件被物化

#### Scenario: 不存在的引导路径静默跳过
- **WHEN** `agent.bootstrap_paths` 含仓库中不存在的路径（如 `.codex`）
- **THEN** workspace 创建成功、无报错，该路径仅作为 sparse 模式常驻

### Requirement: agent.command 为注入交互式 shell 的命令行
`agent.command` SHALL 为单个字符串，以 `herdr pane run <pane> <command>` 方式注入左 pane 的交互式 shell 执行。值支持 shell 别名、函数与环境变量展开。MUST NOT 提供数组形式。

#### Scenario: 别名解析
- **WHEN** 用户 shell rc 中定义了 `alias co=codex`，config.toml 定义 `agent.command = "co"`
- **THEN** 新建工作区时左 pane 经由 shell 别名成功启动 codex

### Requirement: 预留键位不在本 change 实现
`jj.command`（jj 可执行路径解析）与 `agent.auto_trust`（信任弹窗自动应答）SHALL NOT 在本 change 实现：配置文件注释示例中仅以占位注释标注，由后续 change（jj-path-resolution / agent-decoupling）定义。用户若写入这些键，解析 MUST 因未知键而失败。

#### Scenario: 占位键被硬拒绝
- **WHEN** 用户在 config.toml 中写入 `jj.command = "/usr/bin/jj"`
- **THEN** 插件以"未知键 jj.command"错误拒绝运行（该键由 jj-path-resolution change 引入后此场景作废）

> 注：`jj.command` 已由 change `jj-path-resolution` 定义并实现，现为合法键；本场景自该 change 起作废，仅保留历史记录。`agent.auto_trust` 仍为占位。

### Requirement: jj.command 键与取值形态
配置 SHALL 支持 `[jj] command` 键，取值为字符串（可执行文件路径或裸名，不含参数）或字符串数组（完整 argv，其中仅 argv[0] 为可执行文件）。默认值 SHALL 为 `"jj"`。

#### Scenario: 默认值
- **WHEN** config.toml 中未定义 `jj.command`
- **THEN** 插件按裸名 `"jj"` 在 PATH 中查找并使用查得的绝对路径执行所有 jj 调用

#### Scenario: argv 形式仅解析首元素
- **WHEN** `jj.command = ["/opt/jj/bin/jj", "--at-op", "@-"]`
- **THEN** 插件校验 `argv[0]`（`/opt/jj/bin/jj`）为可执行文件并原样使用，其余元素在调用时原样拼接

### Requirement: jj.command 路径解析规则
对 `jj.command`（或 argv 形式的 argv[0]）：以 `/` 开头的绝对路径 SHALL 原样使用；不含 `/` 的裸名 SHALL 在插件进程的 `PATH` 环境变量中按目录顺序查找（条目存在 + 普通文件 + 可执行位，目录缺失或空段静默跳过）；`~` 开头的值 SHALL 先做 `~` 展开再按上述规则处理。

#### Scenario: 裸名 PATH 查找
- **WHEN** `jj.command = "jj"`，且 PATH 中 `/opt/tools/bin` 目录下存在可执行的 `jj`
- **THEN** 插件解析出 `/opt/tools/bin/jj` 并以该绝对路径执行所有 jj 调用

#### Scenario: PATH 中跳过不可执行的同名条目
- **WHEN** PATH 前部某目录存在同名 `jj` 但无可执行位，其后目录存在可执行的 `jj`
- **THEN** 插件跳过不可执行条目，解析到其后目录中的可执行文件

#### Scenario: ~ 展开后按规则处理
- **WHEN** `jj.command = "~/.local/bin/jj"` 且该文件存在且可执行
- **THEN** 插件展开为绝对路径并原样使用

### Requirement: 相对路径拒绝
包含 `/` 但不以 `/` 开头的 `jj.command` 值（如 `./bin/jj`、`bin/jj`）MUST 被拒绝（fail-fast），错误信息 SHALL 说明只接受裸名（无 `/`，按 PATH 查找）或绝对路径。

#### Scenario: 相对路径报错
- **WHEN** `jj.command = "./bin/jj"`
- **THEN** 插件拒绝运行（wizard 界面显示错误 / action 非零退出），错误信息指出相对路径不被接受

### Requirement: 可执行检测失败时 fail-fast 并给出修复指引
解析或校验失败（裸名在 PATH 中找不到、绝对路径不存在/非普通文件/不可执行）时，插件 MUST 以与 config-foundation 相同的 fail-fast 通道失败（wizard TUI 渲染错误；headless action 非零退出）。错误信息 MUST 包含：配置值原文、（裸名时）已搜索的全部 PATH 目录列表、修复建议（"在你的 shell 中运行 `which jj`，将结果写入 config.toml 的 `jj.command`"）。argv 形式的错误信息 SHALL 注明错误来自 `argv[0]`。

#### Scenario: 裸名找不到时的错误内容
- **WHEN** `jj.command = "jj"` 且 PATH 所有目录中均无可执行 `jj`
- **THEN** wizard 打开时界面显示错误并列出已搜索的目录；`new`/`remove` action 以非零码退出，toast 显示失败；错误文本含 `which jj` 修复建议

#### Scenario: 绝对路径不可执行
- **WHEN** `jj.command = "/opt/jj/jj"` 且该文件存在但无可执行位
- **THEN** 插件失败，错误信息指明该路径不可执行

### Requirement: 解析唯一性——一次解析处处使用
插件 SHALL 在每次调用的入口（wizard / remove）对 `jj.command` 解析恰好一次并在进程内贯穿使用：进程内同步 `jj` 调用直接使用解析结果；右侧 pane setup 命令串中的全部 `jj` 调用 SHALL 以解析出的绝对路径烘焙（sh 转义语义不变）。插件 MUST NOT 在右侧 pane 命令串中保留依赖 pane shell PATH 的裸 `jj`。

#### Scenario: 右侧 pane 命令使用绝对路径
- **WHEN** `jj.command` 解析为 `/opt/tools/bin/jj`，用户通过 wizard 创建工作区
- **THEN** 右侧 pane 收到的 setup 命令串中，sparse 物化 / bookmark 创建 / fetch / rebase 均以 `/opt/tools/bin/jj` 调用，不含裸 `jj`

#### Scenario: 插件进程与 pane shell 的 PATH 差异不影响结果
- **WHEN** server PATH 中无 jj（已通过 `jj.command` 显式配置），右侧 pane 用户 shell 的 PATH 中有另一个 `jj`
- **THEN** 所有 jj 调用（含右侧 pane）均使用 `jj.command` 解析出的绝对路径，pane shell 的 PATH 不参与解析

### Requirement: jj.base_rev 的仓库级解析链
`jj.base_rev` 的有效值 SHALL 按 `jj config get herdr.base-rev`（cwd = 选定 source 的仓库根；命令非零退出视为未设置）> config.toml `jj.base_rev` > 内置默认 `trunk()` 的链序解析。`jj config get` 成功但输出为空 SHALL 视为显式空值——非法配置，提交时报错于 wizard 面板内（用户可当场编辑 base 字段继续），不静默回退（与 config.toml 层的空值语义一致；验收反馈补充）。入口预填阶段遇解析错误 MUST NOT 拒绝打开 wizard（display-only：回退全局默认展示）。解析结果 MUST 同时作为 `jj workspace add -r` 的父修订与右侧 setup 脚本的 rebase 目标（`rebase -s @ -d <解析值>`）；`jj git fetch` MUST 保持全量拉取不变。

#### Scenario: 仓库级配置覆盖全局
- **WHEN** 仓库 A 通过 `jj config set --repo herdr.base-rev dev@origin` 设置，config.toml 定义 `base_rev = "main@origin"`
- **THEN** 在仓库 A 创建工作区时基于 `dev@origin`，rebase 目标为 `dev@origin`

#### Scenario: 未设置时回退
- **WHEN** 仓库 B 未在 jj config 任何层设置 `herdr.base-rev`，config.toml 未定义 `jj.base_rev`
- **THEN** 行为与现状逐字节一致：基于 `trunk()` 创建并 rebase 到 `trunk()`
#### Scenario: 仓库级值为空字符串时报错于 wizard 面板内

- **WHEN** 用户执行 `jj config set --repo herdr.base-rev ""` 后在向导中提交创建
- **THEN** 提交时在 wizard 错误行呈现错误（指引设置合法 revset 或删除该键），wizard 不退出、用户可编辑 base 字段继续；入口预填阶段遇此错误不失败（display-only，回退全局默认展示）——不静默回退到 config.toml / `trunk()`

#### Scenario: jj 用户级配置生效
- **WHEN** 用户以 `jj config set --user herdr.base-rev <revset>` 设置个人全局默认，仓库级与 config.toml 均未定义
- **THEN** 解析链取该用户级值（`jj config get` 合并 repo/user 层）

### Requirement: rebase 目标与 base_rev 同源
右侧 setup 脚本的 rebase 目标参数 MUST 为解析链结果，MUST NOT 硬编码 `'trunk()'` 字面量。revset 经逐参数 shell 引用传入脚本、脚本内以双引号引用执行，含空格与引号的 revset MUST 安全传递。

#### Scenario: revset 含空格与引号
- **WHEN** 解析链结果为 `description("x'y") | dev@origin`
- **THEN** `workspace add -r` 与脚本 rebase 均收到完整 revset 单词，脚本执行 `rebase -s @ -d 'description("x'"'"'y") | dev@origin'` 语义等价的原生命令

### Requirement: agent 就绪参数与信任策略可配置

`[agent]` 节 SHALL 支持 4 个新键：`auto_trust`（bool，默认 `false`）、`trust_window_secs`（整数秒，默认 `10`）、`startup_timeout_secs`（整数秒，默认 `20`）、`poll_interval_ms`（整数毫秒，默认 `200`）。`auto_trust` 的占位状态转正：此前写入被硬拒绝的场景自本 change 起作废。

#### Scenario: 默认不自动应答

- **WHEN** 用户未配置 `auto_trust`（或配置为 `false`）
- **THEN** 插件不自动应答任何 agent 的 blocked 状态；blocked 一律以 `"{label} needs attention"` toast 通知用户

#### Scenario: opt-in 自动信任仅对 codex 生效

- **WHEN** 用户配置 `auto_trust = true`，检测标签为 `claude`（非 codex）且 status 为 `blocked`
- **THEN** 插件不发送任何按键，按通用 blocked 语义处理

#### Scenario: 时间下限被拒绝

- **WHEN** 用户配置 `poll_interval_ms = 0` 或 `startup_timeout_secs = 0` 或 `trust_window_secs = 0`
- **THEN** 解析以带键名的错误拒绝（防忙轮询/立即超时退化）

#### Scenario: 时间键的类型错误

- **WHEN** 用户配置 `poll_interval_ms = "200"`（字符串）或 `trust_window_secs = true`
- **THEN** 解析以带键名的类型错误拒绝

### Requirement: finish-tab 接入配置加载

`finish-tab` 子命令 SHALL 在启动时 fail-fast 加载 config.toml（与 `open`/`remove` 一致）：解析错误、未知键、空值 → stderr + 非零退出。

#### Scenario: finish-tab 的配置错误

- **WHEN** config.toml 存在未知键，且 herdr 触发 `finish-tab`
- **THEN** 进程打印带键名的解析错误并以非零码退出，不进入等待循环

