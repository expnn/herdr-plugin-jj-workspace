# Proposal: agent-decoupling

## Why

The plugin's post-creation flow (`finish-tab`) is hardcoded to a single agent, Codex:

- 信任弹窗检测靠屏幕文案爬取（"Do you trust the contents of this directory?" + 选项行），命中后 `pane send-keys enter`；
- 就绪判定靠另一段文案爬取（"OpenAI Codex" + "Ask Codex to do anything" 稳定 1s）；
- `agent list` 过滤硬编码 `agent == "codex"`，失败 toast 硬编码 "Codex needs attention"。

文案爬取是脆弱的 UI 爬虫：任何一端改文案即失效，且对其它 agent（claude/opencode/gemini 等 ~23 个 herdr 可检测的 agent）完全无语义。herdr 原生的 `agent_status` 状态机（`herdr agent list` 输出恒为 JSON envelope）已经提供了所需的全部信号——本次重构把判定来源整体迁移到状态机信号，删除全部屏幕爬取，使插件对 agent 品牌诚实。

## What Changes

- **BREAKING**：`agent.auto_trust` 从占位转正为真实键，默认 `false`——存量 codex 用户不再被自动回车信任弹窗，改为 toast 通知手动回车；想要旧行为需显式 `auto_trust = true`。
- 新增配置：`agent.trust_window_secs`（默认 10）、`agent.startup_timeout_secs`（默认 20）、`agent.poll_interval_ms`（默认 200）。
- `finish-tab` 改为分层就绪状态机：agent list 中该 pane 出现条目即聚焦；`blocked` 持续超 grace 才 toast `"{label} needs attention"`。
- auto-trust 收敛为 codex-only + 启动时间窗门控，并以状态转移确认成功。
- 删除：全部屏幕文案爬取、`read_pane_text`、硬编码 "Codex" 文案。
- 内部常量（不进 config）：`blocked_grace_secs = 1`、`trust_max_attempts = 5`。

## Impact

- Affected specs: `plugin-config`（ADDED：4 个键与硬拒绝语义；MODIFIED：`auto_trust` 占位转正）、`agent-readiness`（NEW 能力）
- Affected code: `src/main.rs` 的 `wait_for_codex_and_accept_trust` / `read_pane_text` / `cmd_finish_tab`；`AgentConfig` 结构体与 README
- 范围外：事件驱动重构（`pane.agent_status_changed` 与 finish-tab 长驻进程模型冲突，见 design）、agent 启动命令解析（`agent.command` 既有行为不变）、herdr 检测 manifest 自身
