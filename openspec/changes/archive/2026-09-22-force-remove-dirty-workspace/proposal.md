## Why

删除 workspace 的对话框把「工作副本干净」当作硬门禁：`jj diff --summary -r @` 非空即 ✗，`↵` 被 `review_blocking_reason` 阻断，用户只能 `c` 提交或退出自己跑 `jj restore`（`workspace-removal` spec 明令对话框 MUST NOT 执行 restore）。但副 workspace 的常见用途是**一次性试验**：验证不成立时整个工作副本（含未提交改动）都应被遗弃，改动本就不会合并到主仓库，强制要求先提交只是把可丢弃的试验内容写进共享存储。缺一个「明知有未提交改动仍要删除」的安全出口。

## What Changes

- Checks 区新增可勾选的 `force remove` 行（仅当 clean 检查**成功检测到脏**时出现），`space` 切换。
- 勾选后按 `↵`：走既有授权前复检 → 仍脏则进入新的**警告页**（`DialogMode`），逐条展示待放弃的未提交改动与后果说明，需**再按一次 `↵`** 才授权；`esc` 返回审阅、零变更。
- 强制删除**仅绕过「工作副本干净」门禁**，不执行任何破坏性 jj 命令：脏 `@` 在 `jj workspace forget` 后以匿名 commit 留在共享存储，工作目录文件随目录删除。改动不会进入主仓库。
- **不绕过 fail-closed**：`jj diff` 调用失败（仓库损坏等）仍然阻断，force 不可用。
- `RemovePlan` 记录 `forced`，Status 的删除步骤标注「forced: abandoned N uncommitted change(s)」。
- force 是瞬态授权：任何刷新（复检 / 对话框内 `c` commit）后重置为未勾选，必须重新确认。
- **非目标**：不改变 opencode session 迁移、pane 关闭、目录删除的既有语义与顺序；不让对话框执行 `jj restore`；不改变干净工作副本的删除路径（无 force 行、无警告页）。

## Capabilities

### New Capabilities

_无_

### Modified Capabilities

- `workspace-removal`: 「移除前拒绝脏工作副本」要求改为「默认拒绝、可显式强制放行」——新增 force 绕过的语义边界（只绕过成功的脏检测、不绕过检测失败）、警告与二次确认作为授权前提、以及脏 `@` 的去向说明。
- `workspace-removal-dialog`: 新增「强制删除开关与二次确认」要求（Checks 区 force 行的出现条件与切换、警告页的内容与键位、force 的瞬态重置）；「审阅界面结构」的 Checks 描述与「键位与视觉一致性」的 hint 文案相应扩展。

## Impact

- `src/main.rs`：`ReviewData` 增加 force 相关状态；`build_review_rows` 增加 force 行与出现条件；`review_blocking_reason` 在 force 开启且检测成功为脏时不再阻断；`run_remove_dialog` 事件循环新增警告页 `DialogMode` 与键位分支；`RemovePlan`/`to_plan` 携带 `forced`；`run_remove_pipeline` + Status 文案标注 forced；hint 常量扩展。
- 单元测试：force 行出现条件、force 不绕过检测失败、警告页二次确认、esc 返回、瞬态重置、plan 携带 forced 的纯函数用例。
- 不涉及 jj 调用形态变更、config 结构、setup 脚本或外部接口。
