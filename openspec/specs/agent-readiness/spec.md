# agent-readiness Specification

## Purpose
TBD - created by archiving change agent-decoupling. Update Purpose after archive.
## Requirements
### Requirement: 分层就绪判定

`finish-tab` SHALL 以 `herdr agent list`（JSON envelope）为唯一判定来源，通过轮询实现分层就绪：目标 pane 出现 agent 条目即视为就绪并可聚焦；`agent_status == "blocked"` 持续超过 grace（内部常量 1s）才判定为需关注；agent 尚未出现时继续轮询直至 `startup_timeout_secs` 预算耗尽。插件 MUST NOT 通过读取 pane 屏幕文本（`pane read`）判定 agent 状态或就绪。

#### Scenario: 条目出现即聚焦

- **WHEN** 轮询到 agent list 中目标 pane 出现条目（任意 agent_status，如 `Working`）
- **THEN** 插件立即执行 focus 循环（workspace/tab/agent focus）并成功退出，不等待任何稳定期或特定状态

#### Scenario: agent 未出现直至超时

- **WHEN** 目标 pane 在 `startup_timeout_secs` 预算内始终没有 agent 条目
- **THEN** 插件以 `"{label} needs attention"` toast（label 为 `unknown` 或最近检测标签）通知并以非零码退出

#### Scenario: 瞬时 blocked 不误报

- **WHEN** 某次轮询读到 `blocked` 但在下一次轮询（间隔 `poll_interval_ms`）已回到非 blocked
- **THEN** 不触发需关注判定（grace 未满）

### Requirement: 自动信任门控

当且仅当以下条件全部成立时，插件 SHALL 对左 pane 发送 `enter`：`agent.auto_trust == true`；运行时检测标签 `== "codex"`；`agent_status == "blocked"`；处于启动窗口内（自 finish-tab 启动起 < `trust_window_secs`）。回车后 SHALL 以状态转移确认：回到非 blocked 则继续；持续 blocked 则重试，不超过内部常量上限（5 次）；超限或 `auto_trust = false` 时不自动应答，按通用 blocked 语义处理。

#### Scenario: 三重门控缺一即不回车

- **WHEN** 以下任一不成立：auto_trust 为 true / 标签为 codex / 窗口未过期
- **THEN** 对 blocked 不发送任何按键

#### Scenario: 窗口外 codex blocked 不被自动应答

- **WHEN** `auto_trust = true`，标签为 `codex`，status 为 `blocked`，但已过 `trust_window_secs`
- **THEN** 不回车，按通用 blocked 语义处理

#### Scenario: 回车以状态转移确认

- **WHEN** 窗口内 codex blocked 被回车后，下一轮询 status 仍为 `blocked`
- **THEN** 在 5 次上限内再次回车；超过上限后按通用 blocked 语义 fail

### Requirement: 通知与文案对 agent 品牌诚实

需关注 toast 的标题 SHALL 为 `"{label} needs attention"`，`label` 取自 agent list 的运行时检测标签；检测不到条目时用 `unknown`。插件 MUST NOT 硬编码任何特定 agent 品牌名于 finish-tab 的用户可见文案。

#### Scenario: toast 携带运行时标签

- **WHEN** 检测标签为 `claude` 的 agent 触发需关注
- **THEN** toast 标题为 `claude needs attention`

### Requirement: 已删除的爬屏通道

屏幕文案匹配（信任提示文案、agent 就绪文案）与 `pane read` 调用 SHALL 自本 change 起从 finish-tab 路径中移除；`src/` 中 MUST NOT 残留上述文案字符串（grep 完成判据）。

#### Scenario: 零爬屏残留

- **WHEN** 在 `src/` 中 grep "Ask Codex"、"Do you trust"、"read_pane_text"
- **THEN** 零命中

