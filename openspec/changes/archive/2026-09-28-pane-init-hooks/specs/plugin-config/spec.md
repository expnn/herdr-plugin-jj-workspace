# plugin-config Delta

## ADDED Requirements

### Requirement: [init] 节为两个 pane 提供初始化命令前缀

`[init]` 节 SHALL 支持三个可缺席的字符串键：`default`、`left`、`right`。键存在时值 MUST 非空——显式空串按既有"解析失败时 fail-fast"规则拒绝，错误定位到具体键名（如 `init.left`）；"未配置"只能由键缺席表达。值 SHALL 为 shell 命令行：原文注入该 pane 的交互式 shell（与 `agent.command` 同一注入模型），MUST NOT 提供数组形式，插件 MUST NOT 对该值做转义或二次引用。

每个 pane SHALL 独立解析：左 pane 取 `init.left`，缺席时取 `init.default`；右 pane 取 `init.right`，缺席时取 `init.default`；两者皆缺席 = 该 pane 不做任何初始化。显式键 MUST 覆盖 `init.default`（覆盖语义，非叠加、非拼接）。

解析结果非空时，该 pane 的命令注入 SHALL 保持单次 `herdr pane run`，注入文本为 `<解析结果> && <该 pane 原命令>`（左 pane 原命令 = `agent.command`；右 pane 原命令 = workspace-setup-script 定义的单次脚本调用）。该 pane 主命令 MUST 仅在初始化命令成功（退出码 0）后执行；初始化失败 MUST NOT 回退到 `init.default`（`default` 是"缺席默认"，不是"失败兜底"）。解析结果为空的 pane，其注入文本 MUST 与本要求引入前逐字节一致（不产生任何 `&&` 或多余空白）。

两个钩子 MUST 都在该 pane 主命令之前、且在全量 checkout（右 pane 脚本的 `jj sparse set --clear --add .`）之前执行；因此钩子执行时新工作副本只有引导文件，本要求不承诺项目文件已存在。**checkout 完成后的初始化不在本要求范围内**。

#### Scenario: 全缺席时行为逐字节不变
- **WHEN** config.toml 无 `[init]` 节（或三键均缺席），用户通过 wizard 创建 jj 工作区
- **THEN** 左 pane 注入文本逐字节等于 `agent.command`，右 pane 注入文本为原有单次脚本调用（无 `&&` 前缀）

#### Scenario: default 同时作用于两个 pane
- **WHEN** `[init] default = "export HTTPS_PROXY=http://proxy:8080"`，`left` 与 `right` 均缺席
- **THEN** 左 pane 注入 `export HTTPS_PROXY=http://proxy:8080 && <agent.command>`，右 pane 注入 `export HTTPS_PROXY=http://proxy:8080 && <脚本调用>`（各自仍为单次 pane run）

#### Scenario: 显式键覆盖 default
- **WHEN** `default = "true"`、`left = "source .venv/bin/activate"`
- **THEN** 左 pane 注入 `source .venv/bin/activate && <agent.command>`（`default` 对左 pane 不参与）；右 pane 注入 `true && <脚本调用>`

#### Scenario: 单侧配置不影响另一侧
- **WHEN** 仅 `left = "sh ~/init.sh"`，`default` 与 `right` 缺席
- **THEN** 左 pane 注入 `sh ~/init.sh && <agent.command>`；右 pane 注入文本不变

#### Scenario: 初始化失败阻断该 pane 主命令
- **WHEN** `init.right` 解析为一条非零退出的命令（如 `false`）
- **THEN** 右 pane 不执行 setup 脚本（错误输出留在该 pane，脚本未被拉起）；左 pane 的 agent 启动不受影响；且不尝试 `init.default`

#### Scenario: 左 pane 初始化失败则不启动 agent
- **WHEN** `init.left` 解析为一条非零退出的命令（如 `false`）
- **THEN** 左 pane 不执行 `agent.command`；失败输出留在该 pane，后续由既有 agent-readiness 超时语义处理（finish-tab 提示未检测到 agent）；不尝试 `init.default`

#### Scenario: init 值原文注入
- **WHEN** `init.left = "export A='x y'"`
- **THEN** 注入文本中该片段逐字符原样出现（不做 `sh_c_escape` 或二次引用），由 pane 的交互式 shell 解析

#### Scenario: 空串键被拒绝
- **WHEN** config.toml 写入 `[init] left = ""`
- **THEN** 插件拒绝运行，错误信息定位到 `init.left` 且说明不允许空值

#### Scenario: 未知键沿用既有拒绝规则
- **WHEN** config.toml 写入 `[init] lef = "x"`
- **THEN** 插件按未知键规则拒绝运行，错误信息指出 `init.lef`

## MODIFIED Requirements

（无）

## REMOVED Requirements

（无）
