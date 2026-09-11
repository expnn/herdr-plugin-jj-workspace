## ADDED Requirements

### Requirement: 枚举并迁移 workspace 绑定的全部 session

迁移步骤 SHALL 在工作目录被删除前，以范围比较（`directory = :ws OR (directory >= :ws || '/' AND directory < :ws || '0')`）而非 LIKE 枚举绑定在 workspace 根目录及其子目录下的全部 session 行（含子 agent session——各占独立行），并在单个写事务中将各行的 `project_id` 设为目标值、`directory` 设为主仓库根、`path` 设为空串（统一拍平）、`workspace_id` 设为 NULL、`time_updated` 设为当前毫秒。

#### Scenario: 根与子目录 session 全部迁移

- **WHEN** DB 中存在 `directory` 等于 workspace 根及位于其子目录下的 session 行，迁移执行
- **THEN** 这些行的 `project_id` 变为主仓库目标、`directory` 变为主仓库根、`path` 为空串、`workspace_id` 为 NULL，且全部更新在同一事务内生效

#### Scenario: 相似命名的兄弟目录不被误命中

- **WHEN** 存在名称与待删 workspace 仅下划线/连字符之差的兄弟目录（如 `workspace-test-proj_id` 与 `workspace-test-proj-id`），其下有 session
- **THEN** 枚举不匹配该兄弟目录的 session（不存在 LIKE 下划线通配误命中）

#### Scenario: 子 agent session 一并被迁移

- **WHEN** workspace 下存在子 agent 产生的 session（`parent_id` 非空的独立行）
- **THEN** 该行同样被枚举并迁移

### Requirement: 主仓库根解析 fail-closed

迁移步骤 SHALL 由待删 workspace 的 `.jj/repo` 指针文件解析主仓库根（跟随指针、去掉尾部 `.jj/repo`、canonicalize）；解析失败或结果与 workspace 自身相同 SHALL 拒绝删除（fail-closed），MUST NOT 以 workspace 自身充当主仓库继续迁移。

#### Scenario: 正常解析出主仓库根

- **WHEN** `.jj/repo` 为文件且其指针指向可解析的主仓库 store 路径
- **THEN** 得到主仓库根（canonical 绝对路径），作为目标 `directory` 与 project 解析链的输入

#### Scenario: 解析失败时拒绝

- **WHEN** `.jj/repo` 指针缺失/不可读，或解析结果与 workspace 自身相同
- **THEN** 迁移拒绝执行、整个删除被拒绝（不执行 forget/目录删除/tab 关闭），消息说明原因

### Requirement: 目标 project 只用 opencode 既有答案解析

迁移步骤 SHALL 按以下顺序解析主仓库目标 `project_id`，且 MUST NOT 自行复刻哈希计算：(0) 主仓库无 `.git`（纯 jj）→ `'global'`；(1) 读 `<主仓库>/.git/opencode` 缓存文件并确认 `project` 表存在对应行；(2) 按 `worktree = 主仓库` 查询 `project` 表；(3) 均不满足 → 拒绝删除并在消息中指引"先在主仓库打开一次 opencode 后重试"。

#### Scenario: 纯 jj 主仓库解析为 global

- **WHEN** 主仓库无 `.git`
- **THEN** 目标为 `'global'`，迁移仅更新 directory 等列（session 原本就是 global）

#### Scenario: 缓存文件优先于按行查询

- **WHEN** `<主仓库>/.git/opencode` 存在且 `project` 表有对应行
- **THEN** 使用缓存文件中的 id 作为目标

#### Scenario: 不可解析时 fail-closed

- **WHEN** 缓存文件与 `project` 表行均不存在
- **THEN** 删除被拒绝（不执行 forget/目录删除/tab 关闭），消息包含可操作指引

### Requirement: 写前 schema 探测 fail-closed

迁移步骤 SHALL 在执行任何写入前探测 DB schema：`session` 表必需列 {id, project_id, directory, path, workspace_id, time_updated} 与 `project` 表必需列 {id, worktree} 全部存在方可继续；任一缺失 SHALL 拒绝删除，MUST NOT 以缩减写法降级执行或强行写入。

#### Scenario: schema 不符即拒绝

- **WHEN** opencode 升级后 `session` 表缺少任一必需列
- **THEN** 删除被拒绝，DB 数据保持原样，拒绝消息说明原因

### Requirement: 跳过与拒绝的边界

迁移步骤 SHALL 在下列情形静默跳过（不阻塞删除流程）：PATH 上无 `opencode` 二进制；`opencode db path` 失败或 DB 文件不存在；枚举计数为 0。除上述外的失败（DB 打不开、枚举 SQL 失败、schema 探测失败、目标 project 不可解析）SHALL 拒绝整个删除。

#### Scenario: opencode 未安装时跳过

- **WHEN** PATH 上找不到 `opencode`
- **THEN** 迁移步骤跳过，remove 流程照常执行

#### Scenario: 无匹配 session 时跳过

- **WHEN** 枚举计数为 0
- **THEN** 迁移步骤跳过，remove 流程照常执行

#### Scenario: DB 打不开时拒绝

- **WHEN** DB 文件存在但无法打开（如损坏）
- **THEN** 删除被拒绝（fail-closed，宁误拒不误删）

### Requirement: 与运行中 opencode 的并发安全

迁移 SHALL 以与 opencode 自身一致的并发参数打开 DB（WAL、`busy_timeout=5000`、`foreign_keys=ON`），以 `BEGIN IMMEDIATE` 获取写锁并在单事务内完成全部 UPDATE；持锁超时或事务失败 SHALL 拒绝删除且不留部分写入。

#### Scenario: 与活 opencode 进程并发

- **WHEN** 有 opencode 进程正在运行并写入同一 DB
- **THEN** 迁移在 busy_timeout 内成功完成，或整体拒绝（无部分写入）

#### Scenario: 外键兜底拒绝非法 project_id

- **WHEN** 因任何原因产生非法 `project_id` 写入
- **THEN** 该写入被 SQLite 外键约束拒绝，事务回滚

### Requirement: 迁移结果上报

迁移成功（N ≥ 1）时 SHALL 以 toast 上报迁移数量与目标主仓库；跳过情形（opencode 缺失或 N=0）MUST NOT 产生打扰性提示。

#### Scenario: 成功迁移上报数量

- **WHEN** N ≥ 1 个 session 迁移成功
- **THEN** toast 报告迁移数量与目标主仓库路径

#### Scenario: 跳过时静默

- **WHEN** opencode 未安装或无匹配 session
- **THEN** 无 toast、无错误输出，remove 流程不受影响
