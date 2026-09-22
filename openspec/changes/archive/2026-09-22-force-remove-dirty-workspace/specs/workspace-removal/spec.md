## MODIFIED Requirements

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
