## MODIFIED Requirements

### Requirement: name 字段组件级编辑

wizard 的 name 字段 SHALL 以组件级编辑语义处理输入。编辑状态 SHALL 为三态：Fresh（name 等于自动生成的默认值 `workspace/<slug>`）、Prefixed（slug 已清、仅剩前缀）、Free（自由编辑）。组件级操作（整删 slug、整删前缀、首字符替换 slug）SHALL 只在 Fresh/Prefixed 锚点态生效；一旦发生任何字符输入，或任何光标导航键（`←` `→` `Home` `End` `Delete`）按下，SHALL 进入 Free 态，Backspace SHALL 恒为逐字符删除。

- Fresh 态：首字符输入 SHALL 保留 `workspace/` 前缀、仅替换 `<slug>`；Backspace SHALL 删除 `<slug>`、保留前缀（进入 Prefixed 态）。光标导航键 SHALL NOT 修改 name 文本，但 SHALL 将状态置为 Free 并生效光标移动（不比较光标是否实际位移）。
- Prefixed 态：字符输入 SHALL 在 `workspace/` 后追加；Backspace SHALL 清空整个 name（进入 Free 态）——即用户连续两次 Backspace 即可清除前缀。光标导航键同 Fresh 规则：不修改文本、进入 Free。
- Free 态：字符 SHALL 在光标处插入；Backspace SHALL 删除光标前一个字符；`Delete` SHALL 删除光标处字符；`←`/`→` SHALL 左右移动一个字符位置；`Home`/`End` SHALL 将光标移至行首/行尾。
- 光标 SHALL 以字符（char）下标表示，初始位于末尾，并在字段间切换时保留；字段未聚焦时 SHALL NOT 渲染光标，聚焦时 SHALL 以块字符渲染于光标所在位置。
- 组件级删除 SHALL 只作用于自动生成默认值衍生的锚点内容；用户手输的名字（含多级 `/`，如 `feature/foo`）SHALL 恒为逐字符编辑。
- 提交校验 SHALL 保持 `valid_branch`（`[A-Za-z0-9._/-]`）不变，空名仍被拒绝。

#### Scenario: 首字符输入保留前缀替换 slug
- **WHEN** name 字段为自动生成的 `workspace/brave-river-0000` 且处于 Fresh 态，用户输入 `f`
- **THEN** name 变为 `workspace/f`（前缀保留、slug 被替换），进入 Free 态

#### Scenario: Free 态继续输入追加
- **WHEN** name 为 `workspace/f`（Free 态，光标在末尾）时用户继续输入 `ix`
- **THEN** name 变为 `workspace/fix`（光标在末尾时插入等价于追加）

#### Scenario: 一次 Backspace 删除 slug 保留前缀
- **WHEN** name 字段为自动生成的 `workspace/brave-river-0000`（Fresh 态），用户按一次 Backspace
- **THEN** name 变为 `workspace/`（进入 Prefixed 态）

#### Scenario: 两次 Backspace 清除前缀
- **WHEN** name 为 `workspace/`（Prefixed 态）时用户再按一次 Backspace
- **THEN** name 变为空字符串（进入 Free 态），用户可输入无前缀的名字

#### Scenario: Free 态逐字符删除
- **WHEN** name 为 `workspace/fix-ap1`（Free 态，光标在末尾），用户按一次 Backspace
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

#### Scenario: 导航键退出 Fresh 锚点且不破坏默认名
- **WHEN** name 字段为自动生成的 `workspace/brave-river-0000`（Fresh 态），用户按一次 `←` 后再按一次 Backspace
- **THEN** 按 `←` 后 name 文本保持不变（仅光标左移一个字符、进入 Free 态）；随后的 Backspace 只删除光标前的一个字符，不整删 slug

#### Scenario: Free 态在光标处插入
- **WHEN** name 为 `workspace/fx`（Free 态，光标位于 `x` 之前），用户输入 `i`
- **THEN** name 变为 `workspace/fix`

#### Scenario: Delete 前向删除光标处字符
- **WHEN** name 为 `workspace/fix`（Free 态，光标位于 `x` 之前），用户按一次 `Delete`
- **THEN** name 变为 `workspace/fi`

#### Scenario: Home 与 End 移动光标到首尾
- **WHEN** name 为 `workspace/fix`（Free 态，光标在末尾），用户按 `Home` 后再输入 `a`
- **THEN** 光标移至行首，name 变为 `aworkspace/fix`

#### Scenario: 光标块渲染于光标位置
- **WHEN** name 为 `workspace/fix`（Free 态，光标位于 `x` 之前）且 name 字段聚焦
- **THEN** 渲染文本为 `workspace/fi█x`（块字符位于光标所在位置，而非行尾）

## ADDED Requirements

### Requirement: base 字段光标编辑

wizard 的 base 字段 SHALL 支持与 name 字段 Free 态一致的单行光标编辑。字段未编辑时 SHALL 保持"首次按键替换整值"语义：首个字符输入或 Backspace SHALL 先清空预填值再应用；任意光标导航键（`←` `→` `Home` `End` `Delete`）按下 SHALL 取消该替换语义且 SHALL NOT 修改文本、SHALL NOT 使字段变为 dirty，此后字符在光标处插入、Backspace 删除光标前一字符、`Delete` 删除光标处字符、`←`/`→`/`Home`/`End` 移动光标。光标 SHALL 以字符（char）下标表示，初始位于末尾，并在字段间切换时保留；字段未聚焦时 SHALL NOT 渲染光标，聚焦时 SHALL 以块字符渲染于光标所在位置。dirty 判定与提交时的解析链求值 SHALL 保持「base 字段与 dirty 惰性解析」不变。

#### Scenario: 首次按键替换整值
- **WHEN** base 预填 `trunk()` 且未被编辑，用户输入 `d`
- **THEN** base 变为 `d`（替换整值而非追加）

#### Scenario: 导航键取消替换语义
- **WHEN** base 预填 `trunk()` 且未被编辑，用户按一次 `←` 后输入 `d`
- **THEN** base 变为 `trunk(d)`（文本未被清空，字符插入在光标处）

#### Scenario: 导航后 Backspace 不整删
- **WHEN** base 预填 `trunk()`，用户按两次 `←` 后再按一次 Backspace
- **THEN** base 变为 `trun()`（仅删除光标前一字符）

#### Scenario: Home 后前向删除
- **WHEN** base 为 `trunk()`（光标在末尾），用户按 `Home` 后按一次 `Delete`
- **THEN** base 变为 `runk()`

#### Scenario: 导航不使字段 dirty
- **WHEN** base 预填 `trunk()` 且未被编辑，用户仅按导航键移动光标后直接提交
- **THEN** 解析链仍按当前 source 现算（字段未被编辑）

#### Scenario: 切换字段保留光标位置
- **WHEN** 用户在 base 中把光标移到中间，Tab 切到 name 再 Tab 回 base
- **THEN** base 的光标位置保持不变
