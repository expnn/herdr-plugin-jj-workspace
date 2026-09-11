# workspace-removal Specification

## Purpose
TBD - created by archiving change lifecycle-docs. Update Purpose after archive.
## Requirements
### Requirement: 移除前拒绝脏工作副本

`remove` action SHALL 在执行 `jj workspace forget` 之前，以 `jj diff --summary -r @`（cwd = 待删工作副本，使用已解析的 `jj.command`）检测工作副本是否有未提交修改；输出非空时 SHALL 拒绝执行删除（stderr + `herdr notification show` toast 说明：检测到未提交修改、已提交内容与书签不受影响、出路为先 `jj commit` 或 `jj restore`），且 MUST NOT 执行 forget、目录删除或 tab 关闭。`jj diff` 调用本身失败时 SHALL 同样拒绝（fail-closed：宁可误拒不误删）。

#### Scenario: 脏工作副本被拒绝

- **WHEN** 工作副本存在未提交修改（`jj diff --summary -r @` 输出非空），用户触发 `remove`
- **THEN** 插件打印带出路的拒绝消息并以非零码退出；工作副本目录、仓库状态与 herdr tab 均保持原样

#### Scenario: 干净工作副本照常移除

- **WHEN** 工作副本无未提交修改（输出为空）
- **THEN** `workspace forget`、目录删除、tab 关闭按原流程执行

#### Scenario: 检测失败时 fail-closed

- **WHEN** `jj diff` 调用失败（如仓库损坏）
- **THEN** 拒绝执行删除（不把检测失败当作"干净"）

#### Scenario: 已提交内容不受影响是拒绝消息的一部分

- **WHEN** 拒绝发生
- **THEN** 消息明确说明已提交的 commit 与书签在共享存储中安全（不会被 remove 丢失）

### Requirement: 移除前迁移 opencode session

`remove` action SHALL 在 `jj workspace forget` 之前执行 opencode session 迁移步骤（详见 `opencode-session-migration` 能力）：迁移被拒绝（fail-closed）时整个 remove MUST NOT 继续执行——不执行 forget、目录删除或 tab 关闭，目录与数据保持原样以便重试；迁移跳过（opencode 缺失或无匹配 session）或成功时，后续流程照常。

#### Scenario: 迁移失败阻止删除

- **WHEN** 迁移步骤因任何 fail-closed 情形被拒绝
- **THEN** 不执行 forget、目录删除、tab 关闭；stderr + toast 说明原因与出路，工作目录完好可重试

#### Scenario: 迁移成功后照常移除

- **WHEN** 迁移步骤成功（N ≥ 1）或被跳过
- **THEN** `jj workspace forget`、目录删除、tab 关闭按原流程执行

#### Scenario: 迁移位于 clean 检查之后

- **WHEN** remove 流程依次执行
- **THEN** 顺序为：目录/仓库守卫 → `check_remove_clean` → opencode session 迁移 → forget → 目录删除 → tab 关闭

