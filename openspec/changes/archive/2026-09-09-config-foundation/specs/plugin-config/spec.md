# Delta Spec: plugin-config

## ADDED Requirements

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
