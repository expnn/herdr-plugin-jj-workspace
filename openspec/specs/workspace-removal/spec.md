# workspace-removal Specification

## Purpose
TBD - created by archiving change lifecycle-docs. Update Purpose after archive.

## Requirements

### Requirement: 移除前拒绝脏工作副本

删除流程 SHALL 在执行任何破坏性步骤（迁移、forget、目录删除、pane 关闭）前确认工作副本干净：对话框打开时以 `jj diff --summary -r @`（cwd = 待删工作副本，使用已解析的 `jj.command`）检测并作为 Checks 项展示；`↵` 授权时 SHALL 再次复检。脏工作副本 SHALL 默认阻断，`↵` MUST NOT 执行任何步骤并回到审阅状态；**唯一放行路径**是用户在审阅中显式开启强制删除并完成二次确认（见 `workspace-removal-dialog` 的「强制删除开关与二次确认」要求），此时 SHALL 跳过 clean 门禁继续执行其余步骤。强制删除 MUST NOT 执行任何额外的破坏性 jj 命令（尤其 MUST NOT 执行 `jj restore`）；未提交改动随工作目录删除，其脏 `@` 以匿名 commit 留在共享存储中、不会进入主仓库。`jj diff` 调用失败 SHALL 无条件阻断（fail-closed：宁可误拒不误删），且 SHALL NOT 因强制删除而放行。脏时 SHALL 展示出路指引（保留修改 `jj commit -m` 可在对话框内授权执行；丢弃修改 `jj restore` 仅展示不执行）；「已提交内容与书签在共享存储中安全、不会被 remove 丢失」的说明 SHALL 保留在对话框内（Plan 第 2 项与阻断提示）。

#### Scenario: 脏工作副本被拒绝

- **WHEN** 工作副本存在未提交修改、未开启强制删除且用户尝试确认删除
- **THEN** 不执行迁移、forget、目录删除或 pane 关闭；对话框显示阻断原因与出路（`c` commit / `jj restore` 指引）

#### Scenario: 强制删除放弃未提交改动

- **WHEN** 工作副本存在未提交修改，用户显式开启强制删除并通过二次确认
- **THEN** 不提交、不 restore，直接执行迁移 → forget → 目录删除 → 关闭勾选 pane；未提交改动随工作目录删除，脏 `@` 以匿名 commit 留在共享存储，不进入主仓库

#### Scenario: 干净工作副本照常移除

- **WHEN** 工作副本无未提交修改（检测输出为空）
- **THEN** 对话框可授权并执行完整删除流程（迁移 → forget → 目录删除 → 关闭勾选 pane）

#### Scenario: 检测失败时 fail-closed

- **WHEN** `jj diff` 调用失败（如仓库损坏）
- **THEN** 视为阻断（不把检测失败当作"干净"），且强制删除 SHALL NOT 放行

#### Scenario: 已提交内容不受影响是拒绝消息的一部分

- **WHEN** 用户审阅或遇到阻断
- **THEN** 提示明确说明已提交的 commit 与书签在共享存储中安全（不会被 remove 丢失）

#### Scenario: 确认时复检

- **WHEN** 对话框打开时工作副本干净，但用户确认前在别处产生新修改
- **THEN** 未开启强制删除时确认被阻断（复检发现脏），不执行任何步骤

### Requirement: 移除前迁移 opencode session

删除流程 SHALL 在 `jj workspace forget` 之前执行 opencode session 迁移（详见 `opencode-session-migration` 能力），且仅迁移用户在对话框中勾选的 session；迁移被拒绝（fail-closed）时整个删除 MUST NOT 继续——不执行 forget、目录删除或 pane 关闭，目标目录与数据保持原样以便重试；迁移跳过（opencode 缺失、无匹配 session、用户未勾选任何 session）或成功时，后续流程照常。

#### Scenario: 迁移失败阻止删除

- **WHEN** 迁移因任何 fail-closed 情形被拒绝
- **THEN** 不执行 forget、目录删除、pane 关闭；Status 显示原因与出路，目标目录完好可重试

#### Scenario: 迁移成功后照常移除

- **WHEN** 迁移成功（勾选子集全部迁移完成）
- **THEN** `jj workspace forget`、目录删除、关闭勾选 pane 按原流程执行

#### Scenario: 迁移位于 clean 检查之后

- **WHEN** remove 流程依次执行
- **THEN** 顺序为：目标解析/守卫 → `↵` 时 clean 复检 → session 迁移 → forget → 目录删除 → 关闭勾选 pane

#### Scenario: 未勾选任何 session 视为跳过

- **WHEN** 用户在审阅中取消勾选全部 session
- **THEN** 迁移步骤跳过（不视为失败），后续流程照常

#### Scenario: stale 目标跳过目录删除

- **WHEN** 直接目标目录在执行前被删（路径已知）
- **THEN** 目录删除步骤跳过，session 迁移与 pane 关闭按已知路径照常

#### Scenario: picker 的 missing on disk 目标仅 forget

- **WHEN** 目标来自 picker 的 missing on disk 项（路径未记录，`root` 为空）
- **THEN** 仅执行 `jj workspace forget`；session 迁移、目录删除与 pane 关闭均跳过（路径未知）并在 Status 中标记

### Requirement: 删除目标解析与守卫

删除目标 SHALL 为聚焦 pane 的 cwd 上溯得到的最近 jj workspace 根（副 workspace 本身，不消解到主仓库根）。主 workspace 自身 MUST NOT 被删除；在主 workspace 上下文触发时 SHALL 进入副 workspace 选择流程（见 `workspace-removal-dialog` 能力）。非 jj 目录、路径不存在或不安全路径（`/`、无父级）SHALL 被拒绝且不产生任何变更。

#### Scenario: 副 workspace 子目录触发

- **WHEN** 聚焦 pane 的 cwd 位于副 workspace 的子目录
- **THEN** 删除目标为该副 workspace 根

#### Scenario: 主 workspace 不可作为删除目标

- **WHEN** 聚焦 pane 位于主 workspace
- **THEN** 目标不解析为主 workspace 自身，改由 picker 选择具体副 workspace

### Requirement: 按 pane 选择关闭（替代 tab 关闭）

删除流程 SHALL 只关闭用户在对话框中勾选的 pane，MUST NOT 使用 `herdr tab close` 或其它按 tab 关闭的接口。候选 pane SHALL 为所有 herdr workspace 中 cwd 或 foreground_cwd 位于目标目录内（含目录本身与子路径，须做路径边界比较）的 pane；cwd 在目录外的 pane SHALL NOT 被关闭。tab 与 herdr workspace 的关闭 SHALL 由 herdr 的「最后一个 pane 关闭即关 tab、最后一个 tab 关闭即关 workspace」级联自然发生。单个 pane 的关闭失败 SHALL 仅警告，不阻断其余 pane 的关闭与流程结束；成功执行时未勾选的 pane SHALL 保持运行（其 cwd 将失效的警示由对话框呈现）。

#### Scenario: 同 tab 异目录 pane 存活

- **WHEN** 目标 tab 内存在 cwd 已在其它目录的 pane 且用户未勾选它
- **THEN** 该 pane 与所在 tab 保持存活（tab 因仍有 pane 而不关闭）

#### Scenario: 全部 pane 被勾选时 tab 级联关闭

- **WHEN** 用户勾选某 tab 的全部 pane 并执行
- **THEN** 关闭这些 pane 后该 tab 自然关闭，目标目录下不再有 pane 残留

#### Scenario: 对话框自身 pane 的处理

- **WHEN** 删除执行到关闭 pane 阶段
- **THEN** 对话框自身 overlay pane 不在候选中、不被显式关闭，随进程退出自然关闭

### Requirement: 执行结果与失败语义

执行中任一破坏性步骤失败 SHALL 停留在对话框 Status 视图，说明失败步骤与已完成/未执行的部分，并给出 `error.log` 指针；成功时对话框自动退出。失败 MUST NOT 继续执行后续步骤（迁移失败不 forget；forget 失败不删目录；目录删除失败不关 pane）。

#### Scenario: forget 失败不关 pane

- **WHEN** `jj workspace forget` 失败
- **THEN** 目录与 pane 保持不变，Status 报告失败与后续未执行

#### Scenario: 成功以 Status 收尾

- **WHEN** 全部步骤成功
- **THEN** 结果在 Status 视图呈现后自动退出（不再依赖 toast 作为唯一出口）
