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

