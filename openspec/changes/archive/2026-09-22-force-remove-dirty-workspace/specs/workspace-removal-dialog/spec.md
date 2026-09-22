## ADDED Requirements

### Requirement: 强制删除开关与二次确认

审阅界面的 Checks 区 SHALL 在「工作副本干净」检查**成功检测到脏**（`jj diff` 成功且输出非空、目标路径已知）时提供一个可选中的强制删除行（文案含将放弃的改动数，如 `force remove (abandon N uncommitted changes)`）；干净或检测失败时该行 SHALL NOT 出现。`space` 在强制删除行上 SHALL 切换其开启状态。

开启强制删除后按 `↵` SHALL 先执行既有授权前复检：复检后仍脏时 SHALL 进入**强制删除警告页**而非直接删除；复检后已干净时 SHALL 按常规直接放行（不进入警告页、不显示警告）。警告页 SHALL 展示目标路径、待放弃的未提交改动（沿用 clean 检查的逐行展示与截断规则）以及后果说明：未提交改动不会进入主仓库、脏 `@` 以匿名 commit 留在共享存储、工作目录将被删除。警告页键位 SHALL 为 `↵ confirm force remove · esc back`：`↵` SHALL 授权带 `forced` 标记的删除流程，`esc` SHALL 返回审阅且 MUST NOT 产生任何变更。

强制删除 SHALL 为瞬态授权：任何一次检查刷新（授权前复检、对话框内 `c` commit 成功后的刷新）之后 SHALL 重置为未开启，需要用户重新显式开启与确认。检测失败的 ✗ SHALL NOT 因强制删除而放行。

#### Scenario: 脏时才出现强制行

- **WHEN** 审阅界面打开且工作副本有未提交修改
- **THEN** Checks 区出现强制删除行；用户按 `space` 后其状态变为开启

#### Scenario: 干净或检测失败时无强制行

- **WHEN** 工作副本干净，或 `jj diff` 调用失败
- **THEN** 审阅界面不出现强制删除行

#### Scenario: 警告页二次确认

- **WHEN** 工作副本脏、强制删除已开启且用户按 `↵`
- **THEN** 进入警告页，展示目标路径与待放弃改动；再按 `↵` 才授权删除流程

#### Scenario: 警告页返回零变更

- **WHEN** 用户在警告页按 `esc`
- **THEN** 返回审阅界面，不执行迁移、forget、目录删除或任何 pane 关闭

#### Scenario: 复检后已干净则直接放行

- **WHEN** 用户开启强制删除后，工作副本在授权前于别处变干净，用户按 `↵`
- **THEN** 复检发现干净，不进入警告页，按常规直接授权删除

#### Scenario: 强制开启状态被刷新重置

- **WHEN** 用户在脏状态开启强制删除并触发一次检查刷新（如 `c` 提交成功）
- **THEN** 强制删除被重置为未开启，需重新开启

## MODIFIED Requirements

### Requirement: 阻断检查与授权修复

带 ✗ 检查时 `↵` MUST NOT 执行删除，并 SHALL 显示阻断原因摘要；唯一例外是「工作副本干净」检查**成功检测到脏**且用户已显式开启强制删除——此时 `↵` 进入强制删除警告页（见「强制删除开关与二次确认」），由二次确认授权，而非直接删除。检测失败（`jj diff` 报错）的 ✗ SHALL 无条件阻断，不受强制删除影响。工作副本脏时 Checks SHALL 逐字展示出路命令（保留修改：`jj commit -m "<message>"`；丢弃修改：`jj restore`），并提供 `c` 子态：单行输入 commit message，`↵` 以目标目录为 cwd、已解析的 `jj.command` 执行 `jj commit -m <message>`；成功后刷新检查状态，失败时在对话框内显示 jj 原生错误。`jj restore`（丢弃性操作）MUST NOT 由对话框执行。opencode DB 不可读（打开失败、schema 不符、目标 project 不可解析）时 SHALL 展示拒绝原因与建议，且不可在对话框内自动修复。

#### Scenario: 脏工作副本阻断

- **WHEN** 工作副本有未提交修改、强制删除未开启且用户按 `↵`
- **THEN** 不执行任何删除步骤，显示阻断摘要与 `c` / `jj restore` 指引

#### Scenario: 对话框内 commit 后放行

- **WHEN** 用户按 `c` 输入 message 并提交且 `jj commit -m <message>` 成功
- **THEN** 检查变为 ✓，随后 `↵` 可正常执行删除

#### Scenario: restore 仅展示

- **WHEN** 工作副本脏且用户查看指引
- **THEN** `jj restore` 仅作为文本展示，不存在任何触发其执行的按键路径

### Requirement: 键位与视觉一致性

对话框 SHALL 采用与创建 wizard 一致的视觉语言（同一 catppuccin palette、section 粗体标题与统一缩进、顶部静态 hint 行、底部居中操作按钮、红色错误/阻断行），弹窗宽度 96（较创建 wizard 宽 10，为长路径加宽；验收调整）；按钮为 `↵ remove`（破坏性配色）与 `esc cancel`。hint 行 SHALL 覆盖当前模式可用键位（picker：`↑↓ move · ↵ select · esc cancel`；review：`↑↓ move · space toggle · a all/none · c commit… · ↵ remove · esc cancel`；强制删除警告页：`↵ confirm force remove · esc back`）。Esc SHALL 在 picker / review 取消并退出，在 commit 子态与强制删除警告页返回审阅；任一返回路径 MUST NOT 产生任何变更。

#### Scenario: 取消无副作用

- **WHEN** 用户按 esc 退出对话框
- **THEN** 不执行迁移、forget、目录删除或任何 pane 关闭

#### Scenario: hint 随模式变化

- **WHEN** 从 picker 进入 review，或从 review 进入强制删除警告页
- **THEN** hint 行切换为对应模式的键位，位置与样式保持一致
