## 1. 共享单行光标编辑原语

- [x] 1.1 实现共享的单行编辑表示（文本 + char 光标）与按键应用函数，覆盖字符插入、Backspace、Delete、`←`/`→`、`Home`/`End`；验证：单元测试覆盖光标在首/尾/中间及空文本的边界用例全部通过
- [x] 1.2 实现光标渲染辅助（聚焦时把块字符插在光标 char 边界、未聚焦不渲染）；验证：单元测试断言 `workspace/fix` 且光标位于 `x` 前时渲染为 `workspace/fi█x`，未聚焦时无块字符

## 2. name 字段：锚点态 × 导航键契约

- [x] 2.1 扩展 `NameKey`/`apply_name_key`（或等价的 `LineEdit` 包装）支持导航键与前向删除：锚点态（Fresh/Prefixed）按下任意导航键不改文本、进入 Free 并生效光标移动，Free 态委托 1.1 原语；验证：既有 8 条状态机测试断言不变并全部通过，新增"导航退出 Fresh 锚点后 Backspace 只删一个字符"用例通过
- [x] 2.2 wizard 事件循环接入 name 字段的 `←` `→` `Home` `End` `Delete` 分支，Tab 切换保留各字段光标；验证：新增用例覆盖"Free 态光标处插入 / Delete 前向删除 / Home 后行首插入"，且 `cargo test` 通过
- [x] 2.3 name 渲染改为块字符插在光标位置；验证：渲染用例断言光标在中间时的可见文本与 1.2 辅助一致

## 3. base 字段：替换锚点 × 导航键

- [x] 3.1 base 接入共享原语：首个字符输入/Backspace 仍替换整值；任意导航键取消替换语义且不修改文本、不置 dirty；光标在 Tab 切换时保留；验证：单元测试覆盖「首次按键替换整值」「导航后输入 `trunk(d)`」「导航后 Backspace 得 `trun()`」「Home 后 Delete 得 `runk()`」「仅导航后提交仍走解析链」五条路径
- [x] 3.2 base 渲染改为块字符插在光标位置；验证：渲染用例断言光标在中间时的可见文本正确

## 4. remove 对话框 commit message 输入

- [x] 4.1 commit 子态接入共享原语：字符在光标处插入、Backspace/Delete/方向键/Home/End 生效；进入子态仍清空 message 且光标置起始；Esc 返回 review 并清空的既有行为不变；验证：单元测试覆盖「光标处插入 `fx`→`fix`」「Delete 得 `fix`」「Home 后行首插入 `wip: fix bug`」「进入子态清空且光标在起始」
- [x] 4.2 commit message 渲染改为块字符插在光标位置；验证：渲染用例断言中间光标时的可见文本

## 5. 集成验证

- [x] 5.1 `cargo build` 与 `cargo test` 全绿，确认无既有用例回归（特别是 name 状态机 8 条与 remove 对话框既有测试）
- [x] 5.2 手动交互验收：wizard 中 Fresh 态按 `←` 后文本不变、Backspace 逐字符；Free 态中间插入/前向删除；base 导航后输入不清空整值；remove commit 子态中间编辑与 `↵` 提交结果正确；hint 行与按钮布局无变化
