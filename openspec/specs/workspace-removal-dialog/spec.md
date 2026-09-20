# workspace-removal-dialog Specification

## Purpose
定义 `remove` action 的审阅式删除对话框：从自身注入的 context 解析删除目标，以 picker / review / status 三种模式呈现删除任务、检查项与可选择的 session/pane 清单，并在用户显式授权后驱动删除流程与状态上报。

## Requirements

### Requirement: 入口与目标解析

`remove` action SHALL 只做预检并把删除执行交给对话框：config 加载失败或 `jj.command` 解析失败时 toast 并退出；从自身 context 的 `focused_pane_cwd` 逐级上溯到最近的含 `.jj` 的祖先目录作为删除目标；找不到目标（非 jj 目录、cwd 缺失）或目标为不安全路径（`/`、无父级）时 toast 并退出，SHALL NOT 打开对话框。预检通过后 SHALL 以 overlay 方式打开 `remove-wizard` pane（与创建 wizard 同族的 plugin pane）；对话框进程 SHALL 用自身注入的 context 重新解析目标，MUST NOT 依赖 action 侧的 `--env` 转发。对话框直达入口（绕过 action）遇到无法解析的目标时 SHALL 渲染 fail-fast TUI modal（同源文案 + error.log 指针）而非静默退出。

#### Scenario: 子目录触发解析到 workspace 根

- **WHEN** 聚焦 pane 的 cwd 在副 workspace 的子目录中触发 remove
- **THEN** 目标解析为该副 workspace 根，对话框打开并进入审阅模式（不再误报 "not a jj workspace"）

#### Scenario: 非 jj 目录触发被 toast 拒绝

- **WHEN** 聚焦 pane 的 cwd 不在任何 jj 仓库内
- **THEN** action 弹 toast 并退出，不打开对话框

#### Scenario: 直达入口的 fail-fast

- **WHEN** 有人直接打开 `remove-wizard` pane 且 context 无可用 cwd
- **THEN** 显示错误 modal（摘要 + error.log 指针），按键关闭

### Requirement: 主 workspace 的副 workspace picker

目标为主 workspace 时对话框 SHALL 进入 picker 模式：枚举该仓库的全部副 workspace 供用户选择（主 workspace 自身 MUST NOT 出现在列表中），选定后进入审阅模式；无副 workspace 时 SHALL 显示空态并仅允许退出。枚举 SHALL 以 `jj workspace list` 的模板输出（`name` 与 `root` 字段，`--ignore-working-copy`、`-R <主仓库根>`，与 cwd 无关）为数据源，MUST NOT 读取 jj 内部存储文件或解析默认文本输出。`root` 为空的条目（路径未记录或目录已删）SHALL 标记为 missing on disk 并允许选中——此类目标 SHALL 仅执行 `jj workspace forget`：工作目录未知，session 迁移、目录删除与 pane 关闭无法定位对象，均跳过并在审阅界面与 Status 中说明。

#### Scenario: 列出全部副 workspace

- **WHEN** 用户在主 workspace 触发 remove 且仓库有 3 个副 workspace
- **THEN** picker 列出 3 项（名称 + 绝对路径），主 workspace 不在列表中

#### Scenario: stale 注册可清理

- **WHEN** 某副 workspace 的目录已被手动删除（`root` 为空）
- **THEN** picker 该项标记 missing on disk；选中后审阅界面显示目录已缺失，执行时仅 forget —— session 迁移、目录删除与 pane 关闭均跳过（路径未知）

#### Scenario: 无副 workspace 的空态

- **WHEN** 主 workspace 没有任何副 workspace
- **THEN** picker 显示空态文案，仅 esc 可退出

### Requirement: 审阅界面结构

审阅模式 SHALL 渲染三个 section：Workspace（目标路径与主仓库根）、Plan（编号任务清单）、Checks（阻断检查）。Plan SHALL 固定为四项并携带可选项的层级列表：`1. migrate opencode sessions`（其下列出绑定的 session，每项含标题、相对目录与时间）；`2. jj workspace forget`（说明 commits 与 bookmarks 留在共享存储）；`3. delete directory`（显示完整路径，stale 时显示 already missing）；`4. close panes`（其下按 herdr workspace → tab → pane 层级列出候选 pane，每项含 pane id、agent 与状态、相对 cwd 路径）。全路径展示 SHALL 将 `$HOME` 前缀缩写为 `~`；「工作副本干净」存在变更时 SHALL 最多列出前 3 行并以 `… N more` 截断（保证 preserve/discard 指引可达）；「opencode sessions」就绪行 SHALL 分两行呈现（第一行计数，第二行以 dim 色缩进显示 `migrate to <缩写路径>`）。Checks SHALL 至少包含「工作副本干净」与「opencode DB 可读」两项，每项以 ✓/✗ 与原因呈现。对话框 SHALL 在内容超出高度时支持滚动，hint 行与操作按钮保持固定可见；内容溢出时 SHALL 显示滚动条；鼠标滚轮 SHALL 滚动内容坐标（MUST NOT 改变当前选择/光标），键盘 `↑↓` 仍用于在可切换行间移动。

#### Scenario: 层级 pane 列表

- **WHEN** 候选 pane 跨两个 herdr workspace、三个 tab
- **THEN** 列表按 workspace → tab → pane 分组渲染，组内行显示 pane id、agent 状态与相对 cwd

#### Scenario: stale 目标的目录项

- **WHEN** 目标目录已不存在（picker 的 missing on disk 项，路径未知）
- **THEN** Plan 显示目录已缺失，session 迁移、目录删除与 pane 关闭标记为跳过，仅 `jj workspace forget` 可执行

### Requirement: 选择模型与默认值

session 列表与 pane 列表 SHALL 默认全选；`↑↓` 在可切换行间移动；`space` 切换当前叶子行、在组头行切换该组全部子项；`a` 在全部可选对象间全局全选/全不选。session 与 pane 的计数 SHALL 在各自列表上方以 `N of M selected` 实时呈现（无叶子或该步骤被跳过时不显示）。取消勾选的 pane SHALL 保持运行；存在未勾选的候选 pane 时对话框 SHALL 显示其 cwd 将失效的警示。全部 pane 均未勾选时 SHALL 允许执行并保持警示（不阻断）。触发本 action 的 pane SHALL 以 (triggered here) 标记但不强制选中。对话框自身的 overlay pane MUST NOT 出现在候选中（以「cwd 等于 plugin_root 且 label 属于本插件 pane 标题」排除），其关闭 MUST 由进程退出自然发生。

#### Scenario: 默认全选

- **WHEN** 对话框打开且共有 3 个 session 与 4 个候选 pane
- **THEN** 两者均默认全部勾选，计数显示 3 of 3 / 4 of 4

#### Scenario: 组头级联切换

- **WHEN** 光标位于某 tab 组头并按 `space`
- **THEN** 该组全部 pane 的勾选状态一起切换，其他组不受影响

#### Scenario: 未勾选警示

- **WHEN** 用户取消勾选 2 个候选 pane
- **THEN** 对话框显示这 2 个 pane 将继续运行但其 cwd 将被删除的警示

### Requirement: 阻断检查与授权修复

带 ✗ 检查时 `↵` MUST NOT 执行删除，并 SHALL 显示阻断原因摘要。工作副本脏时 Checks SHALL 逐字展示出路命令（保留修改：`jj commit -m "<message>"`；丢弃修改：`jj restore`），并提供 `c` 子态：单行输入 commit message，`↵` 以目标目录为 cwd、已解析的 `jj.command` 执行 `jj commit -m <message>`；成功后刷新检查状态，失败时在对话框内显示 jj 原生错误。`jj restore`（丢弃性操作）MUST NOT 由对话框执行。opencode DB 不可读（打开失败、schema 不符、目标 project 不可解析）时 SHALL 展示拒绝原因与建议，且不可在对话框内自动修复。

#### Scenario: 脏工作副本阻断

- **WHEN** 工作副本有未提交修改且用户按 `↵`
- **THEN** 不执行任何删除步骤，显示阻断摘要与 `c` / `jj restore` 指引

#### Scenario: 对话框内 commit 后放行

- **WHEN** 用户按 `c` 输入 message 并提交且 `jj commit -m <message>` 成功
- **THEN** 检查变为 ✓，随后 `↵` 可正常执行删除

#### Scenario: restore 仅展示

- **WHEN** 工作副本脏且用户查看指引
- **THEN** `jj restore` 仅作为文本展示，不存在任何触发其执行的按键路径

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

### Requirement: 授权执行与 Status 视图

`↵` 授权后对话框 SHALL 切换到 Status 视图，按序推进并逐项展示任务状态（迁移 → forget → 删除目录 → 关闭所选 pane）；执行前 SHALL 复检工作副本干净，失败则回到审阅并显示原因，MUST NOT 执行任何步骤。全部成功时 SHALL 自动退出（自身 pane 随进程退出关闭）；任一破坏性步骤（迁移 / forget / 删除目录）失败时 SHALL 停留在 Status 视图，显示失败步骤、已完成/未执行说明与 error.log 指针，按键关闭；逐个关闭 pane 失败 SHALL 仅作警示（不阻断后续 pane 的关闭与退出）。成功执行时 MUST NOT 调用 `herdr tab close`——tab/workspace 的关闭由 herdr 的「最后一个 pane 关闭即关 tab」级联决定。

#### Scenario: 成功自动退出

- **WHEN** 四个任务全部成功
- **THEN** 对话框进程退出、自身 overlay pane 关闭；被勾选的 pane 已全部关闭

#### Scenario: 执行前复检失败

- **WHEN** 用户在审阅后、`↵` 前于别处弄脏了工作副本
- **THEN** 不执行任何删除步骤，回到审阅并说明原因

#### Scenario: 部分 pane 关闭失败

- **WHEN** 关闭某个 pane 被 herdr 拒绝（如 worktree group 保护）
- **THEN** 其余 pane 继续关闭，Status 对该 pane 显示警示，流程正常结束

### Requirement: 键位与视觉一致性

对话框 SHALL 采用与创建 wizard 一致的视觉语言（同一 catppuccin palette、section 粗体标题与统一缩进、顶部静态 hint 行、底部居中操作按钮、红色错误/阻断行），弹窗宽度 96（较创建 wizard 宽 10，为长路径加宽；验收调整）；按钮为 `↵ remove`（破坏性配色）与 `esc cancel`。hint 行 SHALL 覆盖当前模式可用键位（picker：`↑↓ move · ↵ select · esc cancel`；review：`↑↓ move · space toggle · a all/none · c commit… · ↵ remove · esc cancel`）。Esc SHALL 在任意审阅状态取消并退出，且 MUST NOT 产生任何变更。

#### Scenario: 取消无副作用

- **WHEN** 用户按 esc 退出对话框
- **THEN** 不执行迁移、forget、目录删除或任何 pane 关闭

#### Scenario: hint 随模式变化

- **WHEN** 从 picker 进入 review
- **THEN** hint 行从选择键位切换为审阅键位，位置与样式保持一致
