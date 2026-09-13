## ADDED Requirements

### Requirement: 单一源解析

wizard 的 source SHALL 是唯一值：调用者聚焦 pane 的目录经 `jj_root()` 上溯到 jj workspace 根、再经 `repo_root()` 消解副 workspace 到主仓库根。源目录语义为 pane 的 shell cwd（`HERDR_PLUGIN_CONTEXT_JSON.focused_pane_cwd`），SHALL NOT 使用 `foreground_cwd`。两个入口（`cmd_open` action 进程、wizard overlay pane 进程）SHALL 各自读取自身注入的 context，不经自定义 `--env` 转发。`workspace_id`（新 tab 落点）SHALL 同样取自自身 context。

#### Scenario: 子目录上溯到根

- **WHEN** 聚焦 pane 的 cwd 在某 jj 仓库的子目录中
- **THEN** source 为该仓库根，而非子目录

#### Scenario: 副 workspace 消解为主仓库根

- **WHEN** 聚焦 pane 位于某副 workspace 中
- **THEN** source 为其主仓库根（`.jj/repo` 指针规则）；展示、base 解析、`workspace add` cwd、dest 命名共用该值

#### Scenario: 空 context 无法确定源

- **WHEN** 自身 context 无 `focused_pane_cwd`（如 global context 下无 active workspace）
- **THEN** 视为无源，按非 jj 源路径处理（action 侧 toast，wizard 侧 modal）

### Requirement: 非 jj 源预检 toast

`cmd_open`（headless action）SHALL 在打开 wizard pane 之前用自身 context 的 `focused_pane_cwd` 做 jj 预检：`jj_root()` 为空（非 jj 或无源）时 SHALL 调 `die()` 弹 toast（标题 `jj-workspace error`，一行摘要 + `error.log` 指针）并退出，SHALL NOT 打开 wizard pane。

#### Scenario: 非 jj 目录触发 action

- **WHEN** 用户在非 jj 目录的 pane 中触发 New jj workspace action
- **THEN** 弹出 toast 报错，不打开 wizard pane，完整信息写入 `error.log`

#### Scenario: jj 目录触发 action 照常打开

- **WHEN** 用户在 jj 仓库（含子目录、副 workspace）中的 pane 触发 action
- **THEN** 预检通过并打开 wizard pane，Source 区展示消解后的主仓库根

### Requirement: wizard 源 fail-fast

wizard 入口 SHALL 对源做 fail-fast 校验：context 无 `focused_pane_cwd`、路径不存在、或 `jj_root()` 为空时，SHALL 渲染 TUI 错误 modal（与 toast 同源文案：摘要 + `error.log` 指针）并按 enter 退出，SHALL NOT 进入向导主界面。

#### Scenario: 绕过 action 直开 pane

- **WHEN** 有人直接打开 wizard pane entrypoint 且当前聚焦 pane 非 jj
- **THEN** 显示错误 modal 而不是向导，按 enter 关闭

## MODIFIED Requirements

### Requirement: base 字段与 dirty 惰性解析

wizard SHALL 提供第二个可编辑字段 base（Tab 循环 New Workspace Name → Base → New Workspace Name），显示解析链对唯一 source 的预填值。解析链的最终求值 SHALL 在提交时执行：字段未被编辑（dirty = false）时按 source 现算（一次 `jj config get`）；字段被编辑（dirty = true）时直接采用用户输入。

#### Scenario: 未编辑时按 source 求值

- **WHEN** 用户打开 wizard 后未编辑 base 字段直接提交
- **THEN** 解析链按当前 source 现算

#### Scenario: 编辑后采用输入值

- **WHEN** 用户在 base 字段输入 `dev@origin` 并提交
- **THEN** 创建使用 `dev@origin`，解析链不再求值

### Requirement: section 化渲染结构

wizard SHALL 将弹窗内容渲染为四个 section——顺序依次为 New Workspace Name、Base（jj revset）、Source Workspace、Checkout；每个 section SHALL 由独立标题行与内容区组成，相邻 section 之间 SHALL 留有间隔行。标题行 SHALL NOT 包含任何操作提示文字（如 "tab to edit"、"type to filter"）。Source Workspace SHALL 为只读 section（内容为消解后的主仓库根），Checkout SHALL 为只读 section（内容为派生目的路径）；二者 SHALL NOT 进入 tab 焦点循环。

#### Scenario: 标题行不含操作提示

- **WHEN** 渲染任一 section 标题
- **THEN** 该行除标题文字外不包含 "tab"、"filter"、"edit" 等操作提示词

#### Scenario: 只读 section 不可聚焦

- **WHEN** 用户反复按 tab 轮换可编辑字段
- **THEN** 焦点仅在 New Workspace Name、Base 二者间循环，Source Workspace 与 Checkout section 永不获得焦点

### Requirement: 标题聚焦样式统一

所有 section 标题 SHALL 恒以粗体渲染。当前聚焦的可编辑 section 标题 SHALL 以 accent 色渲染，非聚焦 section 标题 SHALL 以 subtext0 色渲染；Source Workspace 与 Checkout 因只读恒为 subtext0 粗体。焦点切换 SHALL 仅改变标题颜色，不改变其粗体属性，也不使标题落入 overlay0/surface_dim 等其他灰色阶。

#### Scenario: 焦点切换只变颜色

- **WHEN** 焦点从 New Workspace Name 切换到 Base
- **THEN** New Workspace Name 标题由 accent 变为 subtext0（恒粗体），Base 标题由 subtext0 变为 accent（恒粗体）

#### Scenario: 只读标题颜色恒定

- **WHEN** 任意字段获得焦点
- **THEN** Source Workspace 与 Checkout 标题始终保持 subtext0 粗体，不随焦点变化

### Requirement: 顶部单一提示行

wizard SHALL 在弹窗标题（"New Workspace"）正下方渲染一行静态操作提示，内容覆盖输入/编辑、tab 切换、提交与取消（如 `type to edit · tab switch · ↵ create · esc cancel`），以 overlay0 渲染，且 SHALL NOT 随焦点字段变化。提示行 SHALL NOT 包含列表选择类操作提示（如 select/filter）。

#### Scenario: 提示行静态且单行

- **WHEN** 用户在不同字段间切换焦点
- **THEN** 顶部提示行内容保持不变，弹窗内无第二处操作提示文字

### Requirement: 内容统一缩进

各 section 的内容行 SHALL 相对标题采用同一缩进列渲染。New Workspace Name、Base 的输入值文本与 Source Workspace 的源路径文本、Checkout 的目的路径文本 SHALL 起始于同一列。

#### Scenario: 各 section 内容对齐

- **WHEN** 同时观察 name 输入值、base 输入值、source 路径与 checkout 路径
- **THEN** 它们的可见文本起始于同一列

## REMOVED Requirements

### Requirement: 候选源仅含 jj workspace

**Reason**: source 候选列表整体删除，被单一源解析（调用者聚焦 pane → 主仓库根）取代；不再存在"多个 herdr workspace 候选并过滤"的概念。
**Migration**: 选其他仓库的方式改为先切到对应 pane 再触发 action；spec 由本 change 的"单一源解析"与"非 jj 源预检 toast"需求覆盖。

### Requirement: 零候选空态渲染

**Reason**: 无候选列表即无零候选态；非 jj 情况在 wizard 打开前由 `cmd_open` toast 拦截，直开路径由 wizard fail-fast modal 覆盖。
**Migration**: 空态渲染与 Tab/Enter 短路逻辑随候选列表一并删除。

### Requirement: 查询无匹配保留结构

**Reason**: 无搜索框即无过滤无匹配态。
**Migration**: 随候选列表一并删除；`no matching workspaces` 文案与 Checkout 占位一并删除。

### Requirement: 空 query 占位提示

**Reason**: source 搜索字段整体删除。
**Migration**: 随候选列表一并删除。
