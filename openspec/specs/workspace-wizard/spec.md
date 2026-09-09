# workspace-wizard Specification

## Purpose
TBD - created by archiving change per-repo-base-rev. Update Purpose after archive.
## Requirements
### Requirement: 候选源仅含 jj workspace
wizard 的 source 候选列表 SHALL 仅包含路径下存在 `.jj` 目录（`is_jj_workspace` 判定）的 herdr workspace；非 jj 项目 SHALL NOT 进入候选，用户无法在 wizard 中选中它们。候选路径 SHALL 统一为 workspace 根（herdr `checkout_path`；缺失时以 active pane cwd 向上归一找到的 `.jj` 根为准），忽略 pane 子目录。过滤后候选为空时，wizard SHALL 照常打开并**折叠为空态**：仅渲染 source 区与 "no jj workspaces" 提示及 esc 提示，name/base/checkout 区与创建按钮 SHALL NOT 渲染；Tab/Enter SHALL 被忽略（不切换字段、不产生错误行），用户按 esc 退出。

#### Scenario: 非 jj workspace 被过滤
- **WHEN** 某 herdr workspace 的解析路径（根或归一后）下不存在 `.jj` 目录
- **THEN** 该 workspace 不出现在 wizard source 列表中，无法被选中或搜索到

#### Scenario: 无 jj workspace 时折叠空态
- **WHEN** 所有 herdr workspace 均非 jj 仓库（过滤后候选为空）
- **THEN** wizard 照常打开，仅渲染 source 区与 "no jj workspaces" 空态提示及 esc 提示；按 Tab/Enter 不产生任何状态变化与错误行；用户按 esc 关闭 wizard 而不执行任何创建

#### Scenario: 子目录启动按根处理
- **WHEN** 用户在仓库子目录的 pane 中启动 wizard，且该 workspace 的 checkout_path 存在
- **THEN** 候选路径为 workspace 根而非 pane 子目录；创建出的新 tab 两个 pane 均位于新建 workspace 根

### Requirement: base 字段与 dirty 惰性解析
wizard SHALL 提供第三个可编辑字段 base（Tab 循环 WorkspaceSearch → Name → Base），显示解析链对初始 source 的预填值。解析链的最终求值 SHALL 在提交时执行：字段未被编辑（dirty = false）时按最终选中的 source 现算（一次 `jj config get`）；字段被编辑（dirty = true）时直接采用用户输入。

#### Scenario: 未编辑时按最终 source 求值
- **WHEN** 用户打开 wizard（初始 source 为仓库 A，预填其解析链值）后切换到仓库 B 并直接提交
- **THEN** 解析链按仓库 B 现算，不使用仓库 A 的预填值

#### Scenario: 编辑后采用输入值
- **WHEN** 用户在 base 字段输入 `dev@origin` 并提交
- **THEN** 创建使用 `dev@origin`，解析链不再求值

### Requirement: revset 提交预校验
wizard SHALL 在 Enter 提交时对**最终 base 值**（无论来自解析链还是用户手输）执行 `jj log -r <expr> --no-graph --limit 0 --no-pager`（cwd = source 仓库）校验 revset；非零退出时 SHALL 将 jj 原生错误信息写入 wizard 错误行并停留 wizard，用户可编辑 base 字段后重新提交。（候选源恒为 jj 仓库，不存在跳过校验的非 jj 分支。）

#### Scenario: 语法错误留在 wizard
- **WHEN** 用户输入 `this is (not valid` 并提交
- **THEN** wizard 错误行显示 jj 的解析错误（含位置定位），wizard 不关闭

#### Scenario: 解析链值非法留在 wizard
- **WHEN** 仓库级 `herdr.base-rev` 设为不存在的 revision（如 `fwggowngggw`），base 字段未编辑直接提交
- **THEN** wizard 错误行显示 jj 原生错误（`Revision ... doesn't exist`），wizard 不关闭、不执行创建；用户可编辑 base 字段后重新提交

#### Scenario: 未知符号被捕获
- **WHEN** 用户输入 source 仓库中不存在的 `nosuchbook@origin` 并提交
- **THEN** wizard 错误行显示 "Revision … doesn't exist"，wizard 不关闭

### Requirement: name 字段组件级编辑

wizard 的 name 字段 SHALL 以组件级编辑语义处理输入。编辑状态 SHALL 为三态：Fresh（name 等于自动生成的默认值 `workspace/<slug>`）、Prefixed（slug 已清、仅剩前缀）、Free（自由编辑）。组件级操作（整删 slug、整删前缀、首字符替换 slug）SHALL 只在 Fresh/Prefixed 锚点态生效；一旦发生任何字符输入进入 Free 态，Backspace SHALL 恒为逐字符删除。

- Fresh 态：首字符输入 SHALL 保留 `workspace/` 前缀、仅替换 `<slug>`；Backspace SHALL 删除 `<slug>`、保留前缀（进入 Prefixed 态）。
- Prefixed 态：字符输入 SHALL 在 `workspace/` 后追加；Backspace SHALL 清空整个 name（进入 Free 态）——即用户连续两次 Backspace 即可清除前缀。
- Free 态：字符输入 SHALL 追加；Backspace SHALL 仅删除最后一个字符。
- 组件级删除 SHALL 只作用于自动生成默认值衍生的锚点内容；用户手输的名字（含多级 `/`，如 `feature/foo`）SHALL 恒为逐字符编辑。
- 提交校验 SHALL 保持 `valid_branch`（`[A-Za-z0-9._/-]`）不变，空名仍被拒绝。

#### Scenario: 首字符输入保留前缀替换 slug
- **WHEN** name 字段为自动生成的 `workspace/brave-river-0000` 且处于 Fresh 态，用户输入 `f`
- **THEN** name 变为 `workspace/f`（前缀保留、slug 被替换），进入 Free 态

#### Scenario: Free 态继续输入追加
- **WHEN** name 为 `workspace/f`（Free 态）时用户继续输入 `ix`
- **THEN** name 变为 `workspace/fix`

#### Scenario: 一次 Backspace 删除 slug 保留前缀
- **WHEN** name 字段为自动生成的 `workspace/brave-river-0000`（Fresh 态），用户按一次 Backspace
- **THEN** name 变为 `workspace/`（进入 Prefixed 态）

#### Scenario: 两次 Backspace 清除前缀
- **WHEN** name 为 `workspace/`（Prefixed 态）时用户再按一次 Backspace
- **THEN** name 变为空字符串（进入 Free 态），用户可输入无前缀的名字

#### Scenario: Free 态逐字符删除
- **WHEN** name 为 `workspace/fix-ap1`（Free 态），用户按一次 Backspace
- **THEN** name 变为 `workspace/fix-ap`（仅删除一个字符，不删除整个 slug）

#### Scenario: 无前缀名字合法提交
- **WHEN** 前缀清除后（name 为空）用户输入 `foo` 并提交
- **THEN** 创建使用 name `foo`（通过 `valid_branch` 校验），不强制 `workspace/` 前缀

#### Scenario: 多级手输名恒逐字符编辑
- **WHEN** 用户手输 name 为 `feature/foo`（Free 态）并按一次 Backspace
- **THEN** name 变为 `feature/fo`（逐字符删除，不删除整个 `foo` 组件）

#### Scenario: 空名仍被拒绝
- **WHEN** name 字段为空（Free 态）时用户直接提交
- **THEN** wizard 错误行显示 "name must match [A-Za-z0-9._/-]"，wizard 不关闭

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

