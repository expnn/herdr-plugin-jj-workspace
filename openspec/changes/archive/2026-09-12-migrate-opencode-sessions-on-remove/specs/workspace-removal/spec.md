## ADDED Requirements

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
