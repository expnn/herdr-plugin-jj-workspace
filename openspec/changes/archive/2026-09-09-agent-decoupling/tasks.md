# Tasks: agent-decoupling

## 1. 配置层

- [x] 1.1 `AgentConfig` 扩展 4 个键：`auto_trust: bool`（默认 false）、`trust_window_secs: u64`（默认 10）、`startup_timeout_secs: u64`（默认 20）、`poll_interval_ms: u64`（默认 200）；`Config::validate` 加下限校验（≥1/≥1/≥10，错误带键名）
- [x] 1.2 单元测试：默认值、4 键解析、类型错误带键名、下限拒绝（0 值）、`auto_trust = "yes"` 之类的类型错误
- [x] 1.3 README：Configuration 示例更新（`auto_trust = false` 转正为新键组，移除"defined by a later change"注释）+ agent-readiness 语义说明（分层就绪、opt-in 自动信任、BREAKING 默认行为变更）

## 2. 就绪状态机

- [x] 2.1 实现 `wait_for_agent_ready(config, left_pane, label_out) -> Outcome`：轮询 `herdr agent list`（JSON envelope，不过滤 agent 标签），分层就绪（条目出现即就绪 / blocked 稳定 ≥ 1s 内部常量 → fail / 超预算 → fail）；返回检测标签供 toast 使用
- [x] 2.2 实现 auto-trust 门控：`auto_trust && label=="codex" && status=="blocked" && elapsed < trust_window_secs` → send-keys enter；状态转移确认（回到非 blocked 继续，持续 blocked 在 5 次内部常量上限内重试，超限 fail）
- [x] 2.3 删除：`wait_for_codex_and_accept_trust` 的文案匹配、"OpenAI Codex"/"Ask Codex to do anything" 判定、`read_pane_text`、agent list 过滤中的 `agent=="codex"`、toast 硬编码 "Codex needs attention"（改 `format!("{label} needs attention")`）
- [x] 2.4 `cmd_finish_tab` 接入 `load_config()` fail-fast（错误 → stderr + 非零退出，与 open/remove 一致）
- [x] 2.5 grep 完成判据：`grep -rn "Ask Codex\|Do you trust\|read_pane_text" src/` 零命中

## 3. 测试（假 herdr 注入）

- [x] 3.1 假 `herdr` 可执行文件 fixture：可控的 `agent list` JSON 输出（条目出现/缺失/状态序列），不碰进程 env
- [x] 3.2 状态机测试：条目出现即聚焦（不等稳定期）；条目缺失至超时 → fail + "unknown needs attention"；瞬时 blocked 不误报（grace）；blocked 稳定 → fail + 带标签 toast 文案
- [x] 3.3 门控测试：默认（auto_trust=false）不回车；auto_trust=true + 非 codex 标签不回车；auto_trust=true + codex + 窗口内回车；回车后 blocked→非 blocked 确认成功；持续 blocked 超上限 fail；窗口外不回车
- [x] 3.4 finish-tab 配置错误 fail-fast 测试

## 4. 验证与验收

- [x] 4.1 `cargo test` 全绿；`cargo build --release` 零警告
- [x] 4.2 手工验收（需真实 herdr 运行时）：codex + `auto_trust=false` 创建工作区（手动回车信任提示、toast 通知）；codex + `auto_trust=true`（自动回车、窗口语义）；非 codex agent（如 claude）创建工作区（toast 带标签、无自动应答）
