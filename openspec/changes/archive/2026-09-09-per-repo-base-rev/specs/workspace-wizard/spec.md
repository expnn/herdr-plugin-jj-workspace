# Delta Spec: workspace-wizard

## ADDED Requirements

### Requirement: base 字段与 dirty 惰性解析
wizard SHALL 提供第三个可编辑字段 base（Tab 循环 WorkspaceSearch → Name → Base），显示解析链对初始 source 的预填值。解析链的最终求值 SHALL 在提交时执行：字段未被编辑（dirty = false）时按最终选中的 source 现算（一次 `jj config get`）；字段被编辑（dirty = true）时直接采用用户输入。

#### Scenario: 未编辑时按最终 source 求值
- **WHEN** 用户打开 wizard（初始 source 为仓库 A，预填其解析链值）后切换到仓库 B 并直接提交
- **THEN** 解析链按仓库 B 现算，不使用仓库 A 的预填值

#### Scenario: 编辑后采用输入值
- **WHEN** 用户在 base 字段输入 `dev@origin` 并提交
- **THEN** 创建使用 `dev@origin`，解析链不再求值

### Requirement: revset 提交预校验
对 jj 源且 base 字段参与提交时，wizard SHALL 在 Enter 提交时对**最终 base 值**（无论来自解析链还是用户手输）执行 `jj log -r <expr> --no-graph --limit 0 --no-pager`（cwd = source 仓库）校验 revset；非零退出时 SHALL 将 jj 原生错误信息写入 wizard 错误行并停留 wizard，用户可编辑 base 字段后重新提交。

#### Scenario: 语法错误留在 wizard
- **WHEN** 用户输入 `this is (not valid` 并提交
- **THEN** wizard 错误行显示 jj 的解析错误（含位置定位），wizard 不关闭

#### Scenario: 解析链值非法留在 wizard
- **WHEN** 仓库级 `herdr.base-rev` 设为不存在的 revision（如 `fwggowngggw`），base 字段未编辑直接提交
- **THEN** wizard 错误行显示 jj 原生错误（`Revision ... doesn't exist`），wizard 不关闭、不执行创建；用户可编辑 base 字段后重新提交

#### Scenario: 未知符号被捕获
- **WHEN** 用户输入 source 仓库中不存在的 `nosuchbook@origin` 并提交
- **THEN** wizard 错误行显示 "Revision … doesn't exist"，wizard 不关闭

### Requirement: 非 jj 源置灰
选中非 jj 工作区时，base 字段 SHALL 保持可见且可聚焦，但以置灰样式渲染，并在现有警告行注明非 jj 源忽略此项；提交时非 jj 源 MUST 跳过 base 校验与传递。

#### Scenario: 非 jj 源忽略 base
- **WHEN** 用户选中一个非 jj 文件夹并提交
- **THEN** 不执行 base 校验，打开该文件夹（与现状一致），base 字段置灰显示
