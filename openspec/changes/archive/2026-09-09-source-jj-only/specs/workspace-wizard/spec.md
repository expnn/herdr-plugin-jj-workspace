# workspace-wizard Delta Spec

## ADDED Requirements

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

## MODIFIED Requirements

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

## REMOVED Requirements

### Requirement: 非 jj 源置灰
**Reason**: 候选源改为仅限 jj workspace（见 ADDED "候选源仅含 jj workspace"），非 jj 项目不再可能被选中，置灰渲染、忽略提示与提交时跳过校验的分支失去存在前提。
**Migration**: 删除 base 字段的非 jj 置灰样式、`source_warning` 警告行与非 jj 提交路径；非 jj 项目由 herdr 原生能力处理（不在本插件内打开）。
