# workspace-wizard Delta Spec

## ADDED Requirements

### Requirement: section 化渲染结构
wizard SHALL 将弹窗内容渲染为四个 section——Source Workspace、New Workspace Name、Base（jj revset）、Checkout；每个 section SHALL 由独立标题行与内容区组成，相邻 section 之间 SHALL 留有间隔行。标题行 SHALL NOT 包含任何操作提示文字（如 "tab to edit"、"type to filter"）。checkout SHALL 为只读 section（内容为派生目的路径），SHALL NOT 进入 tab 焦点循环。

#### Scenario: 标题行不含操作提示
- **WHEN** 渲染任一 section 标题
- **THEN** 该行除标题文字外不包含 "tab"、"filter"、"edit" 等操作提示词

#### Scenario: checkout 不可聚焦
- **WHEN** 用户反复按 tab 轮换可编辑字段
- **THEN** 焦点仅在 Source 列表、New Workspace Name、Base 三者间循环，Checkout section 永不获得焦点

### Requirement: 标题聚焦样式统一
所有 section 标题 SHALL 恒以粗体渲染。当前聚焦的可编辑 section 标题 SHALL 以 accent 色渲染，非聚焦 section 标题 SHALL 以 subtext0 色渲染；checkout 标题因只读恒为 subtext0 粗体。焦点切换 SHALL 仅改变标题颜色，不改变其粗体属性，也不使标题落入 overlay0/surface_dim 等其他灰色阶。

#### Scenario: 焦点切换只变颜色
- **WHEN** 焦点从 Source 列表切换到 New Workspace Name
- **THEN** Source 标题由 accent 变为 subtext0（恒粗体），New Workspace Name 标题由 subtext0 变为 accent（恒粗体）

#### Scenario: checkout 标题颜色恒定
- **WHEN** 任意字段获得焦点
- **THEN** checkout 标题始终保持 subtext0 粗体，不随焦点变化

### Requirement: 顶部单一提示行
wizard SHALL 在弹窗标题（"New Workspace"）正下方渲染一行静态操作提示，内容覆盖输入/编辑、列表选择、tab 切换、提交与取消（如 `type to filter or edit · ↑/↓ select · tab switch · ↵ create · esc cancel`），以 overlay0 渲染，且 SHALL NOT 随焦点字段变化。

#### Scenario: 提示行静态且单行
- **WHEN** 用户在不同字段间切换焦点或过滤列表
- **THEN** 顶部提示行内容保持不变，弹窗内无第二处操作提示文字

### Requirement: 内容统一缩进
各 section 的内容行 SHALL 相对标题采用同一缩进列渲染。source 列表行的选中 marker 槽 SHALL 与内容共享同一前导区：选中行显示 `▸`、未选中行保留同宽空位，列表项文字 SHALL NOT 因选中状态左右位移。New Workspace Name、Base 的输入值文本与 Checkout 的目的路径文本 SHALL 与列表项 label 的起始列对齐。

#### Scenario: 选中行文字不位移
- **WHEN** 用 ↑/↓ 在列表项间移动选中
- **THEN** 所有列表项的 label 文字保持在同一列，无水平跳动，仅 marker 槽内容变化

#### Scenario: 各 section 内容对齐
- **WHEN** 同时观察列表项 label、name 输入值、base 输入值与 checkout 路径
- **THEN** 它们的可见文本起始于同一列

### Requirement: 零候选空态渲染
当候选列表为空（无可选 jj workspace）时，wizard SHALL 照常打开，但仅渲染 source section（标题与空态文案 `no jj workspaces — open herdr's project picker instead`）及 esc 退出按钮；SHALL NOT 渲染 New Workspace Name、Base、Checkout section，也 SHALL NOT 渲染 create 按钮。零候选的打开、空态文案与 Tab/Enter 键短路由 source-jj-only（B）保证；本 Requirement 约束**渲染折叠**为仅 source + esc，并 SHALL 覆盖 B 的"全结构 + 列表区空态文案"渲染。此时顶部提示行 SHALL 随之收敛（不显示 create/tab/select 等无效操作提示，仅保留或隐含 esc 退出语义）。

#### Scenario: 零候选只留 source 与退出
- **WHEN** 所有候选均为非 jj（过滤后为空）且 wizard 打开
- **THEN** 弹窗内只见 source 标题 + 空态文案与 esc cancel 按钮，无 name/base/checkout section、无 create 按钮

#### Scenario: 零候选下按键短路
- **WHEN** 零候选态下用户按 tab 或 enter
- **THEN** 不产生字段切换或创建动作、不显示错误行，弹窗保持，esc 可退出

#### Scenario: 零候选提示行收敛
- **WHEN** 零候选态下 wizard 打开
- **THEN** 顶部提示行不出现 create/tab 等无意义操作提示（或整行替换为仅 esc 退出语义）

### Requirement: 查询无匹配保留结构
当存在候选但当前过滤词无匹配项时，wizard SHALL 在列表区显示 `no matching workspaces` 空态文案，并 SHALL 保留下方 section 的正常渲染：New Workspace Name 与 Base 输入值 SHALL 保持用户独立编辑的值，Checkout 预览 SHALL 显示无匹配占位（`no matching workspace`，因无可选中源可派生目的路径）。

#### Scenario: 过滤无匹配不塌缩布局
- **WHEN** 用户输入一个无匹配的过滤词
- **THEN** 列表区显示 "no matching workspaces"，New Workspace Name、Base、Checkout section 仍渲染，Checkout 预览显示占位而非路径

### Requirement: 空 query 占位提示
当焦点在 source 搜索且 query 为空时，输入条内 SHALL 显示浅色占位文字（如 `filter…`）；焦点离开或 query 非空时 SHALL NOT 显示该占位。

#### Scenario: 聚焦空输入显示占位
- **WHEN** 焦点在 source 搜索、query 为空
- **THEN** 输入条内可见浅色占位文字提示可过滤

#### Scenario: 非聚焦或已输入不显示占位
- **WHEN** 焦点在其他字段，或 query 已有内容
- **THEN** 输入条内不显示占位文字
