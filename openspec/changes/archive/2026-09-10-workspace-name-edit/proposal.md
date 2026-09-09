## Why

新建 workspace 的默认名 `workspace/<slug>` 承载命名空间（workspace 名、书签名、目录 slug 三处同源，与 bookmark 前缀分组惯例一致），但 wizard 的 name 字段采用"整串替换"编辑：首字符输入或 Backspace 会清掉整个默认值，用户想改 slug 部分时被迫重输 `workspace/` 前缀。交互与"保留前缀"的结论相悖——编辑一次就把命名空间丢掉了。

## What Changes

- name 字段编辑从**整串替换**细化为**组件级编辑**：首字符输入保留 `workspace/` 前缀、仅替换自动生成的 `<slug>`；Backspace 先删 `<slug>`、再按一次删掉前缀。
- 引入三态编辑状态机（Fresh → Prefixed → Free）：组件级操作只在未编辑锚点（Fresh/Prefixed）生效；一旦进入自由编辑（Free），Backspace 回归逐字符，保证"改错一个字符"不被破坏性清空。
- 组件级删除只作用于自动生成的前缀/后缀结构；用户自输的多级名（如 `feature/foo`）在 Free 态恒为逐字符编辑。
- base 字段不受影响，保持现状。

## Capabilities

### New Capabilities

_无_

### Modified Capabilities

- `workspace-wizard`: 新增 Requirement「name 字段组件级编辑」——定义 name 字段三态编辑状态机与 Backspace/字符输入的锚点语义。

## Impact

- `src/main.rs`：wizard 事件处理（`KeyCode::Char` / `KeyCode::Backspace` 的 name 分支），`replace_on_type: bool` 扩展为三态编辑状态；`WizardView`/绘制层若需区分锚点态显示则一并调整。
- 单元测试：状态机转移（8 条）与组合路径覆盖。
- 不涉及 jj 调用、config 结构、setup 脚本或外部接口。