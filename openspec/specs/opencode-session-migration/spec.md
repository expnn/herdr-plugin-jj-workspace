# opencode-session-migration Specification

## Purpose

删除 jj workspace 前，把绑定在该 workspace 根目录及其子目录下的 opencode session 迁移到主仓库，避免 session 随目录删除而失联。迁移按 schema 选择活跃表（存在 `session_v2` 时用 v2，否则用 v1 的 `session`），并在目标 project 无法唯一解析或 schema 不兼容时拒绝删除（fail-closed）。

## Requirements

### Requirement: 枚举并迁移 workspace 绑定的全部 session

迁移步骤 SHALL 先按 schema 选择 opencode 的活跃 session 表：DB 中存在 `session_v2` 表时 SHALL 以它作为唯一枚举与更新目标（opencode v2 的写入表），否则 SHALL 使用 `session` 表（v1）。选择 MUST NOT 依赖 `opencode --version` 或任何版本字符串。迁移 SHALL 在工作目录被删除前，以范围比较（`directory = :ws OR (directory >= :ws || '/' AND directory < :ws || '0')`）而非 LIKE 枚举所选活跃表中绑定在 workspace 根目录及其子目录下的全部 session 行（含子 agent session——各占独立行），并仅对用户勾选的 session 行在单个写事务中更新：`project_id` 设为目标值、`directory` 设为主仓库根、`path` 设为空串（统一拍平）、`workspace_id` 设为 NULL、`time_updated` 设为当前毫秒；该列集合与语义 SHALL 与 opencode 官方 session 移动（Moved）投影一致。未勾选的行 MUST NOT 被更新；默认全选即等价于旧行为（迁移全部）。未被选为活跃表的另一张表 MUST NOT 被修改。执行期与预览之间的行集漂移 SHALL 按实际更新的行数上报，不视为失败。

#### Scenario: 只迁移勾选的行

- **WHEN** 预览列出 3 行而用户仅勾选 2 行并执行删除
- **THEN** 仅这 2 行被更新，未勾选行的各列保持原值

#### Scenario: 根与子目录 session 全部迁移

- **WHEN** 所选活跃表中存在 `directory` 等于 workspace 根及位于其子目录下的 session 行且均被勾选
- **THEN** 这些行的 `project_id` 变为主仓库目标、`directory` 变为主仓库根、`path` 为空串、`workspace_id` 为 NULL，且全部更新在同一事务内生效

#### Scenario: 相似命名的兄弟目录不被误命中

- **WHEN** 存在名称与待删 workspace 仅下划线/连字符之差的兄弟目录（如 `workspace-test-proj_id` 与 `workspace-test-proj-id`），其下有 session
- **THEN** 枚举不匹配该兄弟目录的 session（不存在 LIKE 下划线通配误命中）

#### Scenario: 子 agent session 一并被迁移

- **WHEN** workspace 下存在子 agent 产生的 session（`parent_id` 非空的独立行）且被勾选
- **THEN** 该行同样被枚举并迁移

#### Scenario: 存在 session_v2 时只操作 v2 表

- **WHEN** DB 同时存在 `session` 与 `session_v2`，且两表都含有绑定该 workspace 的行
- **THEN** 预览与迁移只针对 `session_v2`，`session` 中同 id 的行保持原值

#### Scenario: 仅 v1 表时保持兼容

- **WHEN** DB 中不存在 `session_v2` 表
- **THEN** 使用 `session` 表枚举与更新，行为与既有 v1 语义一致

### Requirement: 主仓库根解析 fail-closed

迁移步骤 SHALL 由待删 workspace 的 `.jj/repo` 指针文件解析主仓库根（跟随指针、去掉尾部 `.jj/repo`、canonicalize）；解析失败或结果与 workspace 自身相同 SHALL 拒绝删除（fail-closed），MUST NOT 以 workspace 自身充当主仓库继续迁移。

#### Scenario: 正常解析出主仓库根

- **WHEN** `.jj/repo` 为文件且其指针指向可解析的主仓库 store 路径
- **THEN** 得到主仓库根（canonical 绝对路径），作为目标 `directory` 与 project 解析链的输入

#### Scenario: 解析失败时拒绝

- **WHEN** `.jj/repo` 指针缺失/不可读，或解析结果与 workspace 自身相同
- **THEN** 迁移拒绝执行、整个删除被拒绝（不执行 forget/目录删除/tab 关闭），消息说明原因

### Requirement: 目标 project 只用 opencode 既有答案解析

迁移步骤 SHALL 按活跃表版本解析主仓库目标 `project_id`，且 MUST NOT 自行复刻 opencode 的 project id 哈希计算。

- 活跃表为 `session_v2`（v2）：SHALL 以 `worktree = canonical(主仓库根)` 在 `project` 表中精确匹配；恰好一行时使用其 `id`；0 行或多行 SHALL 拒绝删除，并在消息中指引先在主仓库打开一次 opencode 后重试（多行时同时说明目标存在歧义）。MUST NOT 假定纯 jj 主仓库解析为 `'global'`。
- 活跃表为 `session`（v1）：SHALL 按既有顺序解析：(0) 主仓库无 `.git`（纯 jj）→ `'global'`；(1) 读 `<主仓库>/.git/opencode` 缓存文件并确认 `project` 表存在对应行；(2) 按 `worktree = 主仓库` 查询 `project` 表；(3) 均不满足 → 拒绝删除并在消息中指引先在主仓库打开一次 opencode 后重试。

#### Scenario: v2 按 worktree 精确解析

- **WHEN** 活跃表为 `session_v2`，且 `project` 表恰有一行 `worktree` 等于主仓库根的 canonical 路径
- **THEN** 使用该行的 `id` 作为目标 `project_id`

#### Scenario: v2 不假定纯 jj 为 global

- **WHEN** 活跃表为 `session_v2`，主仓库无 `.git`，且 `project` 表没有匹配主仓库根的 `worktree` 行
- **THEN** 删除被拒绝（不写入 `'global'`），消息指引先在主仓库打开一次 opencode

#### Scenario: v2 目标歧义时拒绝

- **WHEN** `project` 表有多行 `worktree` 等于主仓库根
- **THEN** 删除被拒绝，消息说明目标 project 存在歧义

#### Scenario: 纯 jj 主仓库解析为 global

- **WHEN** 活跃表为 `session` 且主仓库无 `.git`
- **THEN** 目标为 `'global'`，迁移仅更新 directory 等列（session 原本就是 global）

#### Scenario: 缓存文件优先于按行查询

- **WHEN** 活跃表为 `session` 且 `<主仓库>/.git/opencode` 存在、`project` 表有对应行
- **THEN** 使用缓存文件中的 id 作为目标

#### Scenario: 不可解析时 fail-closed

- **WHEN** 按对应版本的解析链均无法得到目标 project
- **THEN** 删除被拒绝（不执行 forget/目录删除/tab 关闭），消息包含可操作指引

### Requirement: 写前 schema 探测 fail-closed

迁移步骤 SHALL 在执行任何写入前探测 DB schema：按活跃表选择结果探测对应表——`session_v2` 或 `session` 的必需列 {id, project_id, directory, path, workspace_id, time_updated} 与 `project` 表必需列 {id, worktree} 全部存在方可继续；任一缺失 SHALL 拒绝删除，MUST NOT 以缩减写法降级执行或强行写入。若 DB 中既无 `session_v2` 也无 `session` 表，SHALL 拒绝删除。

#### Scenario: v2 schema 不符即拒绝

- **WHEN** `session_v2` 存在但缺少任一必需列
- **THEN** 删除被拒绝，DB 数据保持原样，拒绝消息说明原因

#### Scenario: schema 不符即拒绝

- **WHEN** 活跃表为 `session` 且缺少任一必需列
- **THEN** 删除被拒绝，DB 数据保持原样，拒绝消息说明原因

#### Scenario: 两张 session 表都不存在时拒绝

- **WHEN** DB 中既无 `session_v2` 也无 `session` 表
- **THEN** 删除被拒绝，而不是按「0 匹配」跳过

### Requirement: 跳过与拒绝的边界

迁移步骤 SHALL 在下列情形静默跳过（不阻塞删除流程）：PATH 与常见安装目录均无 `opencode` 二进制且标准数据目录无 DB 文件（发现缺失）；枚举计数为 0（含用户未勾选任何 session）。除上述外的失败（DB 打不开、枚举 SQL 失败、活跃表 schema 探测失败、目标 project 不可解析或其匹配不唯一）SHALL 拒绝整个删除。特别是：当 `session_v2` 存在而迁移无法按其 schema 执行时，MUST NOT 以「0 匹配」静默跳过。

#### Scenario: opencode 未安装时跳过

- **WHEN** PATH 与常见安装目录均找不到 `opencode`，且标准数据目录没有 DB 文件
- **THEN** 迁移步骤跳过，remove 流程照常执行

#### Scenario: 无匹配 session 时跳过

- **WHEN** 所选活跃表的枚举计数为 0
- **THEN** 迁移步骤跳过，remove 流程照常执行

#### Scenario: DB 打不开时拒绝

- **WHEN** DB 文件存在但无法打开（如损坏）
- **THEN** 删除被拒绝（fail-closed，宁误拒不误删）

#### Scenario: v2 表存在但不兼容时拒绝

- **WHEN** `session_v2` 表存在但缺少必需列
- **THEN** 删除被拒绝，不得以跳过或 0 匹配继续

#### Scenario: 目标 project 歧义时拒绝

- **WHEN** v2 目标解析匹配到多行 `project`
- **THEN** 删除被拒绝，不执行任何写入

### Requirement: 与运行中 opencode 的并发安全

迁移 SHALL 以与 opencode 自身一致的并发参数打开 DB（WAL、`busy_timeout=5000`、`foreign_keys=ON`），以 `BEGIN IMMEDIATE` 获取写锁并在单事务内完成全部 UPDATE；持锁超时或事务失败 SHALL 拒绝删除且不留部分写入。

#### Scenario: 与活 opencode 进程并发

- **WHEN** 有 opencode 进程正在运行并写入同一 DB
- **THEN** 迁移在 busy_timeout 内成功完成，或整体拒绝（无部分写入）

#### Scenario: 外键兜底拒绝非法 project_id

- **WHEN** 因任何原因产生非法 `project_id` 写入
- **THEN** 该写入被 SQLite 外键约束拒绝，事务回滚

### Requirement: 迁移结果上报

迁移成功（N ≥ 1）时 SHALL 在对话框 Status 视图上报本次实际迁移数量与目标主仓库；跳过情形（opencode 可执行文件与标准位置 DB 均不可用、N=0、用户未勾选任何 session）MUST NOT 产生打扰性提示或导致失败。

#### Scenario: 成功迁移上报数量

- **WHEN** N ≥ 1 个 session 迁移成功
- **THEN** Status 显示迁移数量与目标主仓库路径

#### Scenario: 跳过时静默

- **WHEN** opencode 不可用且标准位置无 DB、无匹配 session 或用户未勾选任何 session
- **THEN** Status 将该步骤标记为跳过，流程不受影响

### Requirement: 迁移前的只读预览

对话框打开时迁移步骤 SHALL 提供只读预览：以与迁移一致的范围谓词枚举绑定在 workspace 根及其子目录下的 session 行，返回每行的 `id`、`title`、`directory`、`time_updated`，以及目标主仓库根或拒绝原因。预览 MUST NOT 写入 DB。预览失败（DB 打不开、schema 不符、目标 project 不可解析）SHALL 作为阻断检查呈现（对话框内 ✗ + 原因 + 建议），且 MUST NOT 使执行期的 fail-closed 校验被跳过。DB 定位 SHALL 优先调用 `opencode db path`（可执行文件在 PATH 或常见安装目录 `~/.opencode/bin`、`~/.local/bin`、`/usr/local/bin` 中找到时）；CLI 不可用或其输出无效时 SHALL 回退到标准数据目录 `$XDG_DATA_HOME/opencode/opencode.db`（未设置时 `$HOME/.local/share/opencode/opencode.db`）。预览与执行 SHALL 使用同一发现链；CLI 从不用于写入。可执行文件与以上 DB 位置均不可用时预览 SHALL 报告跳过（无 session 列表），不阻断，且跳过原因 SHALL 指明已尝试的位置。

#### Scenario: 预览列出待迁移 session

- **WHEN** 工作区目录下存在 3 个 session（含子目录）
- **THEN** 预览返回 3 行（id / title / directory / time_updated）并在 Plan 第 1 项下展示

#### Scenario: 预览失败作为阻断项

- **WHEN** DB schema 缺少必需列
- **THEN** Checks 显示 ✗ 与 schema 原因，删除被阻断

#### Scenario: 预览不写库

- **WHEN** 用户打开对话框后按 esc 退出
- **THEN** session 表内容未被预览修改
