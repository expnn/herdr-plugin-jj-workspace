# Delta Spec: plugin-config

## ADDED Requirements

### Requirement: agent 就绪参数与信任策略可配置

`[agent]` 节 SHALL 支持 4 个新键：`auto_trust`（bool，默认 `false`）、`trust_window_secs`（整数秒，默认 `10`）、`startup_timeout_secs`（整数秒，默认 `20`）、`poll_interval_ms`（整数毫秒，默认 `200`）。`auto_trust` 的占位状态转正：此前写入被硬拒绝的场景自本 change 起作废。

#### Scenario: 默认不自动应答

- **WHEN** 用户未配置 `auto_trust`（或配置为 `false`）
- **THEN** 插件不自动应答任何 agent 的 blocked 状态；blocked 一律以 `"{label} needs attention"` toast 通知用户

#### Scenario: opt-in 自动信任仅对 codex 生效

- **WHEN** 用户配置 `auto_trust = true`，检测标签为 `claude`（非 codex）且 status 为 `blocked`
- **THEN** 插件不发送任何按键，按通用 blocked 语义处理

#### Scenario: 时间下限被拒绝

- **WHEN** 用户配置 `poll_interval_ms = 0` 或 `startup_timeout_secs = 0` 或 `trust_window_secs = 0`
- **THEN** 解析以带键名的错误拒绝（防忙轮询/立即超时退化）

#### Scenario: 时间键的类型错误

- **WHEN** 用户配置 `poll_interval_ms = "200"`（字符串）或 `trust_window_secs = true`
- **THEN** 解析以带键名的类型错误拒绝

### Requirement: finish-tab 接入配置加载

`finish-tab` 子命令 SHALL 在启动时 fail-fast 加载 config.toml（与 `open`/`remove` 一致）：解析错误、未知键、空值 → stderr + 非零退出。

#### Scenario: finish-tab 的配置错误

- **WHEN** config.toml 存在未知键，且 herdr 触发 `finish-tab`
- **THEN** 进程打印带键名的解析错误并以非零码退出，不进入等待循环
