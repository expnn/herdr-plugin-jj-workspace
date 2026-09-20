## Why

wizard 的 name/base 字段与 remove 对话框 commit 子态的 message 输入框都是 `String` + 隐式末尾光标：编辑只有 `push`/`pop`（`src/main.rs:117-151`、`2210-2232`、`4176-4185`），事件循环对 `←` `→` `Home` `End` `Delete` 一律落入 `_ => {}` 丢弃（`src/main.rs:2233`）。用户在已输入文本中间无法定位与修正，不符合任何常规单行文本编辑习惯；name 字段的 Fresh/Prefixed 锚点态让问题更突出——自动生成的默认名 `workspace/<slug>` 无法原地逐字符编辑。

## What Changes

- wizard 的 name 与 base 字段获得完整单行光标编辑：`←` `→` 移动一字符、`Home`/`End` 到首/尾、`Delete` 前向删除、字符在光标处插入、`Backspace` 删除光标前一字符；光标以块字符渲染在光标位置（不再是末尾追加）。
- name 字段状态机与光标契约：**任意导航键按下即退出 Fresh/Prefixed 锚点进入 Free**；此后 `Backspace` 恒为逐字符，杜绝"移动光标后整删 slug"的破坏性意外。锚点态原有组件级语义（Fresh+字符输入裁 slug 保留前缀、Fresh+Backspace 删 slug、Prefixed+Backspace 清空）与快捷替换流程不变。
- base 字段：首次按键替换整值（`replace_on_type`）的既有语义保留，但任意导航键按下即取消替换锚点并进入光标编辑；光标位置在字段间 Tab 切换时保留，初始在末尾。
- remove 对话框 commit 子态的 message 输入框：同一套光标编辑键位（进入子态仍清空 message）；wizard 与 commit 的 hint 行文案保持不变。
- **非目标**：不做词级跳转（`Ctrl+←/→`）、不做多行编辑、不改名称生成/`valid_branch` 提交校验/base 解析链、不改锚点态组件级语义本身。

## Capabilities

### New Capabilities

_无_

### Modified Capabilities

- `workspace-wizard`: 修改「name 字段组件级编辑」（Free 态从"末尾追加/删除"改为光标相对编辑；新增导航键语义与"导航退出锚点"规则）；新增「base 字段光标编辑」要求。
- `workspace-removal-dialog`: 新增「commit message 输入的光标编辑」要求（commit 子态 message 输入框的键位与光标语义）。

## Impact

- `src/main.rs`：新增共享的单行光标编辑实现（char 下标光标）；`apply_name_key`/`NameKey` 扩展导航与前向删除（或重构为携带光标的编辑结构）；wizard 事件循环（`2153-2234`）、name/base 渲染（`2325-2356`）、remove 对话框 commit 子态事件（`4175-4186`）与渲染（`3792`）。
- 单元测试：name 状态机既有 8 条路径保持通过，新增"导航退出锚点"与光标相对编辑用例；base/commit message 光标编辑的纯函数用例。
- 不涉及 jj 调用、config 结构、setup 脚本或外部接口；hint 常量（`2541-2543`）与按钮布局不变。
