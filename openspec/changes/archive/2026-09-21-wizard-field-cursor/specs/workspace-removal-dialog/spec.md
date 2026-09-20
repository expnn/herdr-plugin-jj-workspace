## ADDED Requirements

### Requirement: commit message 输入的光标编辑

remove 对话框 commit 子态的 message 输入框 SHALL 支持单行光标编辑：字符在光标处插入、Backspace 删除光标前一字符、`Delete` 删除光标处字符、`←`/`→` 左右移动一个字符位置、`Home`/`End` 移至行首/行尾；光标 SHALL 以块字符渲染于光标所在位置。进入 commit 子态 SHALL 仍清空 message 并将光标置于起始位置；`Esc` 返回 review 并清空 message 的既有行为不变；提交与校验语义（非空校验、`jj commit -m`）SHALL 保持不变。

#### Scenario: 光标处插入
- **WHEN** message 为 `fx` 且光标位于 `x` 之前，用户输入 `i`
- **THEN** message 变为 `fix`

#### Scenario: 前向删除
- **WHEN** message 为 `fixx` 且光标位于最后一个 `x` 之前，用户按一次 `Delete`
- **THEN** message 变为 `fix`

#### Scenario: 行首插入
- **WHEN** message 为 `fix bug`（光标在末尾），用户按 `Home` 后输入 `wip: `
- **THEN** message 变为 `wip: fix bug`

#### Scenario: 进入子态清空且光标在起始
- **WHEN** 用户从 review 按 `c` 进入 commit 子态
- **THEN** message 为空、光标位于起始位置，输入从第一个字符开始
