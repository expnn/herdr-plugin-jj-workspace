# Delta Spec: plugin-config

## ADDED Requirements

### Requirement: jj.base_rev 的仓库级解析链
`jj.base_rev` 的有效值 SHALL 按 `jj config get herdr.base-rev`（cwd = 选定 source 的仓库根；命令非零退出视为未设置）> config.toml `jj.base_rev` > 内置默认 `trunk()` 的链序解析。`jj config get` 成功但输出为空 SHALL 视为显式空值——非法配置，提交时报错于 wizard 面板内（用户可当场编辑 base 字段继续），不静默回退（与 config.toml 层的空值语义一致；验收反馈补充）。入口预填阶段遇解析错误 MUST NOT 拒绝打开 wizard（display-only：回退全局默认展示）。解析结果 MUST 同时作为 `jj workspace add -r` 的父修订与右侧 setup 脚本的 rebase 目标（`rebase -s @ -d <解析值>`）；`jj git fetch` MUST 保持全量拉取不变。

#### Scenario: 仓库级配置覆盖全局
- **WHEN** 仓库 A 通过 `jj config set --repo herdr.base-rev dev@origin` 设置，config.toml 定义 `base_rev = "main@origin"`
- **THEN** 在仓库 A 创建工作区时基于 `dev@origin`，rebase 目标为 `dev@origin`

#### Scenario: 未设置时回退
- **WHEN** 仓库 B 未在 jj config 任何层设置 `herdr.base-rev`，config.toml 未定义 `jj.base_rev`
- **THEN** 行为与现状逐字节一致：基于 `trunk()` 创建并 rebase 到 `trunk()`
#### Scenario: 仓库级值为空字符串时报错于 wizard 面板内

- **WHEN** 用户执行 `jj config set --repo herdr.base-rev ""` 后在向导中提交创建
- **THEN** 提交时在 wizard 错误行呈现错误（指引设置合法 revset 或删除该键），wizard 不退出、用户可编辑 base 字段继续；入口预填阶段遇此错误不失败（display-only，回退全局默认展示）——不静默回退到 config.toml / `trunk()`

#### Scenario: jj 用户级配置生效
- **WHEN** 用户以 `jj config set --user herdr.base-rev <revset>` 设置个人全局默认，仓库级与 config.toml 均未定义
- **THEN** 解析链取该用户级值（`jj config get` 合并 repo/user 层）

### Requirement: rebase 目标与 base_rev 同源
右侧 setup 脚本的 rebase 目标参数 MUST 为解析链结果，MUST NOT 硬编码 `'trunk()'` 字面量。revset 经逐参数 shell 引用传入脚本、脚本内以双引号引用执行，含空格与引号的 revset MUST 安全传递。

#### Scenario: revset 含空格与引号
- **WHEN** 解析链结果为 `description("x'y") | dev@origin`
- **THEN** `workspace add -r` 与脚本 rebase 均收到完整 revset 单词，脚本执行 `rebase -s @ -d 'description("x'"'"'y") | dev@origin'` 语义等价的原生命令
