## MODIFIED Requirements

### Requirement: revset 提交预校验
wizard SHALL 在 Enter 提交时对**最终 base 值**（无论来自解析链还是用户手输）执行 `jj log -r <expr> --no-graph --limit 1 --no-pager`（cwd = source 仓库）校验 revset；非零退出时 SHALL 将 jj 原生错误信息写入 wizard 错误行并停留 wizard；退出为零但 stdout 为空（空集 revset，如 `none()`）时 SHALL 以明确错误信息（提示 base revset 解析为空集）写入错误行并停留 wizard。两种失败均 MUST NOT 执行创建，用户可编辑 base 字段后重新提交。（候选源恒为 jj 仓库，不存在跳过校验的非 jj 分支。）

#### Scenario: 语法错误留在 wizard
- **WHEN** 用户输入 `this is (not valid` 并提交
- **THEN** wizard 错误行显示 jj 的解析错误（含位置定位），wizard 不关闭

#### Scenario: 解析链值非法留在 wizard
- **WHEN** 仓库级 `herdr.base-rev` 设为不存在的 revision（如 `fwggowngggw`），base 字段未编辑直接提交
- **THEN** wizard 错误行显示 jj 原生错误（`Revision ... doesn't exist`），wizard 不关闭、不执行创建；用户可编辑 base 字段后重新提交

#### Scenario: 未知符号被捕获
- **WHEN** 用户输入 source 仓库中不存在的 `nosuchbook@origin` 并提交
- **THEN** wizard 错误行显示 "Revision … doesn't exist"，wizard 不关闭

#### Scenario: 空集 revset 被拒绝
- **WHEN** 用户在 base 字段输入 `none()`（或任何解析为空的 revset）并提交
- **THEN** wizard 错误行提示 base revset 解析为空集，wizard 不关闭、不执行创建（不产生 workspace 注册、目录或半成品残留）
