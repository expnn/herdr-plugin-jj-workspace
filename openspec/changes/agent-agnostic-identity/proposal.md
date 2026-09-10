# Proposal: agent-agnostic-identity

## Why

agent-decoupling 已让插件支持 herdr 能检测的任意 agent，但插件对外的身份声明仍自称使用 Codex（manifest description、README 开篇、脚本与代码注释、误导性变量名），与实际能力不符。同时仓库存在一次作者改名（nathanflurry → expnn）留下的 id 漂移：manifest id 已是 `expnn.jj-workspace`，但 README 5 处与 `plugin_id()` 代码 fallback 仍是 `nathanflurry.jj-workspace`——README 的 keybinding 示例照抄会导致绑定不生效。相关改动均未发布过，无需 version bump，修完后发布即为正确状态。

## What Changes

- **默认值变更**：`agent.command` 内置默认值从 `"codex"` 改为 `"opencode"`（未发布，无存量用户，不构成 BREAKING）。
- **身份措辞 agent 无关化**（7 处）：
  1. `herdr-plugin.toml` description → "Create jj workspaces with your coding agent on the left and a terminal on the right."
  2. `README.md:3` 开篇 tagline → "your coding agent"
  3. `README.md:61` "materializes only Codex's startup instructions" → "the agent's startup instructions"（bootstrap_paths 本就是跨 agent 共用列表）
  4. `README.md:66` "while Codex starts on the left" → "while the agent starts on the left"
  5. `scripts/setup-workspace.sh` 头部注释 "accepts Codex's trust prompt" → 改为准确的 opt-in 描述（auto_trust 默认关闭，codex-only，窗口门控）
  6. `src/main.rs` `cmd_wizard` doc comment "Codex-left / terminal-right" → "agent-left / terminal-right"
  7. `src/main.rs` `start_codex` 变量改名 `start_agent`（它启动的是 `agent.command` 解析出的任意 agent）
- **id 漂移修复**：README 5 处（keybindings ×3、config-dir ×2、error.log 路径）与 `src/main.rs` `plugin_id()` fallback 统一为 `expnn.jj-workspace`。
- **测试命名语义化**：`start_command_defaults_to_codex_when_unset` → `command_defaults_to_opencode_when_unset`（键名旧称 start_command 已更名 command，默认值现为 opencode）。
- **不改**（定案）：
  - `bootstrap_paths` 默认值保持 `["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]`——单一跨 agent 共用列表，`.codex` 是数据不是身份。
  - `auto_trust` 的 codex-only 行为及其全部描述（`main.rs:1172` 门控、doc comments、README "Agent startup handling" 段、agent-readiness spec）——是已定案的行为事实，保留才是真实。
  - `openspec/changes/archive/**` 历史档案不回头改写。
  - 不做 version bump（相关改动从未发布）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `plugin-config`: `agent.command` 内置默认值从 `"codex"` 改为 `"opencode"`（"配置文件不存在时使用默认值"场景与"分节 schema 与内置默认值"要求的默认值清单更新；"与迁移前行为一致"从句随 command 默认值变更一并修正）。

## Impact

- `herdr-plugin.toml`（description）
- `README.md`（开篇 tagline、Quickstart 流程描述、Configuration 示例与键说明、keybinding/config-dir/error.log 的插件 id、默认值说明）
- `scripts/setup-workspace.sh`（头部注释）
- `src/main.rs`（`AgentConfig` 默认值、`start_codex` 改名、`cmd_wizard` doc comment、`start_agent` 旁注释的默认值引用、`plugin_id()` fallback、默认值断言测试 ×2 及测试名）
- `openspec/specs/plugin-config/spec.md`（经 delta spec 归档后：默认值场景与默认值要求）
- 行为影响面：仅"未配置 `agent.command` 时左 pane 启动的命令"从 codex 变为 opencode；显式配置行为、auto_trust、其余一切不变。
