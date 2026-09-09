# workspace-wizard Specification (Delta)

## ADDED Requirements

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