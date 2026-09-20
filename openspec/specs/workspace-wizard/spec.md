# workspace-wizard Specification

## Purpose
TBD - created by archiving change per-repo-base-rev. Update Purpose after archive.
## Requirements
### Requirement: 单一源解析
wizard 的 source SHALL 是唯一值：调用者聚焦 pane 的目录经 `jj_root()` 上溯到 jj workspace 根、再经 `repo_root()` 消解副 workspace 到主仓库根。源目录语义为 pane 的 shell cwd（`HERDR_PLUGIN_CONTEXT_JSON.focused_pane_cwd`），SHALL NOT 使用 `foreground_cwd`。两个入口（`cmd_open` action 进程、wizard overlay pane 进程）SHALL 各自读取自身注入的 context，不经自定义 `--env` 转发。`workspace_id`（新 tab 落点）SHALL 同样取自自身 context。

#### Scenario: 子目录上溯到根
- **WHEN** 聚焦 pane 的 cwd 在某 jj 仓库的子目录中
- **THEN** source 为该仓库根，而非子目录

#### Scenario: 副 workspace 消解为主仓库根
- **WHEN** 聚焦 pane 位于某副 workspace 中
- **THEN** source 为其主仓库根（`.jj/repo` 指针规则）；展示、base 解析、`workspace add` cwd、dest 命名共用该值

#### Scenario: 空 context 无法确定源
- **WHEN** 自身 context 无 `focused_pane_cwd`（如 global context 下无 active workspace）
- **THEN** 视为无源，按非 jj 源路径处理（action 侧 toast，wizard 侧 modal）

### Requirement: 非 jj 源预检 toast
`cmd_open`（headless action）SHALL 在打开 wizard pane 之前用自身 context 的 `focused_pane_cwd` 做 jj 预检：`jj_root()` 为空（非 jj 或无源）时 SHALL 调 `die()` 弹 toast（标题 `jj-workspace error`，一行摘要 + `error.log` 指针）并退出，SHALL NOT 打开 wizard pane。

#### Scenario: 非 jj 目录触发 action
- **WHEN** 用户在非 jj 目录的 pane 中触发 New jj workspace action
- **THEN** 弹出 toast 报错，不打开 wizard pane，完整信息写入 `error.log`

#### Scenario: jj 目录触发 action 照常打开
- **WHEN** 用户在 jj 仓库（含子目录、副 workspace）中的 pane 触发 action
- **THEN** 预检通过并打开 wizard pane，Source 区展示消解后的主仓库根

### Requirement: wizard 源 fail-fast
wizard 入口 SHALL 对源做 fail-fast 校验：context 无 `focused_pane_cwd`、路径不存在、或 `jj_root()` 为空时，SHALL 渲染 TUI 错误 modal（与 toast 同源文案：摘要 + `error.log` 指针）并按 enter 退出，SHALL NOT 进入向导主界面。

#### Scenario: 绕过 action 直开 pane
- **WHEN** 有人直接打开 wizard pane entrypoint 且当前聚焦 pane 非 jj
- **THEN** 显示错误 modal 而不是向导，按 enter 关闭

### Requirement: base 字段与 dirty 惰性解析
wizard SHALL 提供第二个可编辑字段 base（Tab 循环 New Workspace Name → Base → New Workspace Name），显示解析链对唯一 source 的预填值。解析链的最终求值 SHALL 在提交时执行：字段未被编辑（dirty = false）时按 source 现算（一次 `jj config get`）；字段被编辑（dirty = true）时直接采用用户输入。

#### Scenario: 未编辑时按 source 求值
- **WHEN** 用户打开 wizard 后未编辑 base 字段直接提交
- **THEN** 解析链按当前 source 现算

#### Scenario: 编辑后采用输入值
- **WHEN** 用户在 base 字段输入 `dev@origin` 并提交
- **THEN** 创建使用 `dev@origin`，解析链不再求值

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

### Requirement: section 化渲染结构

wizard SHALL 将弹窗内容渲染为四个 section——顺序依次为 New Workspace Name、Base（jj revset）、Source Workspace、Checkout；每个 section SHALL 由独立标题行与内容区组成，相邻 section 之间 SHALL 留有间隔行。标题行 SHALL NOT 包含任何操作提示文字（如 "tab to edit"、"type to filter"）。Source Workspace SHALL 为只读 section（内容为消解后的主仓库根），Checkout SHALL 为只读 section（内容为派生目的路径）；二者 SHALL NOT 进入 tab 焦点循环。

#### Scenario: 标题行不含操作提示
- **WHEN** 渲染任一 section 标题
- **THEN** 该行除标题文字外不包含 "tab"、"filter"、"edit" 等操作提示词

#### Scenario: 只读 section 不可聚焦
- **WHEN** 用户反复按 tab 轮换可编辑字段
- **THEN** 焦点仅在 New Workspace Name、Base 二者间循环，Source Workspace 与 Checkout section 永不获得焦点

### Requirement: 标题聚焦样式统一

所有 section 标题 SHALL 恒以粗体渲染。当前聚焦的可编辑 section 标题 SHALL 以 accent 色渲染，非聚焦 section 标题 SHALL 以 subtext0 色渲染；Source Workspace 与 Checkout 因只读恒为 subtext0 粗体。焦点切换 SHALL 仅改变标题颜色，不改变其粗体属性，也不使标题落入 overlay0/surface_dim 等其他灰色阶。

#### Scenario: 焦点切换只变颜色
- **WHEN** 焦点从 New Workspace Name 切换到 Base
- **THEN** New Workspace Name 标题由 accent 变为 subtext0（恒粗体），Base 标题由 subtext0 变为 accent（恒粗体）

#### Scenario: 只读标题颜色恒定
- **WHEN** 任意字段获得焦点
- **THEN** Source Workspace 与 Checkout 标题始终保持 subtext0 粗体，不随焦点变化

### Requirement: 顶部单一提示行

wizard SHALL 在弹窗标题（"New Workspace"）正下方渲染一行静态操作提示，内容覆盖输入/编辑、tab 切换、提交与取消（如 `type to edit · tab switch · ↵ create · esc cancel`），以 overlay0 渲染，且 SHALL NOT 随焦点字段变化。提示行 SHALL NOT 包含列表选择类操作提示（如 select/filter）。

#### Scenario: 提示行静态且单行
- **WHEN** 用户在不同字段间切换焦点
- **THEN** 顶部提示行内容保持不变，弹窗内无第二处操作提示文字

### Requirement: 内容统一缩进

各 section 的内容行 SHALL 相对标题采用同一缩进列渲染。New Workspace Name、Base 的输入值文本与 Source Workspace 的源路径文本、Checkout 的目的路径文本 SHALL 起始于同一列。

#### Scenario: 各 section 内容对齐
- **WHEN** 同时观察 name 输入值、base 输入值、source 路径与 checkout 路径
- **THEN** 它们的可见文本起始于同一列
