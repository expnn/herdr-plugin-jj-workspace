# Tasks: agent-agnostic-identity

## 1. 代码：默认值与命名

- [x] 1.1 `src/main.rs` `AgentConfig::default()`：`command: "codex".into()` → `"opencode".into()`（约 line 268）
- [x] 1.2 `src/main.rs` `start_codex` 变量改名 `start_agent`（约 line 1027-1029），同步修正旁注释 `(default "codex")` → `(default "opencode")`（约 line 1023）
- [x] 1.3 `src/main.rs` `cmd_wizard` doc comment："Codex-left / terminal-right Herdr workspace." → "agent-left / terminal-right Herdr workspace."（约 line 678）
- [x] 1.4 `src/main.rs` `plugin_id()` fallback：`"nathanflurry.jj-workspace"` → `"expnn.jj-workspace"`（约 line 2101）
- [x] 1.5 测试更新：默认值断言改为 `"opencode"`（config 默认断言与 `resolve_start_command` 默认断言）；测试名 `start_command_defaults_to_codex_when_unset` → `command_defaults_to_opencode_when_unset`；其余以 codex 作示例值/测试数据的测试保持不动

## 2. 文档与身份声明

- [x] 2.1 `herdr-plugin.toml` description → `"Create jj workspaces with your coding agent on the left and a terminal on the right."`（版本号保持 0.5.0 不动）
- [x] 2.2 `README.md` 开篇 tagline（line 3）→ "New tabs open with your coding agent on the left and an interactive terminal on the right."
- [x] 2.3 `README.md` Quickstart 流程描述：line 61 "materializes only Codex's startup instructions" → "the agent's startup instructions"；line 66 "while Codex starts on the left" → "while the agent starts on the left"
- [x] 2.4 `README.md` Configuration 节：示例注释与 `agent.command` 键说明的默认值陈述 "Defaults to `codex`" → "Defaults to `opencode`"（保留 "set it to whatever your shell resolves" 的 agent 无关表述与 auto_trust codex-only 段落原文）
- [x] 2.5 `README.md` id 漂移修复：keybindings ×3、`config-dir` ×2（line 76/226）、error.log 路径（line 239）全部 `nathanflurry.jj-workspace` → `expnn.jj-workspace`
- [x] 2.6 `scripts/setup-workspace.sh` 头部注释（line 10）："accepts Codex's trust prompt" → 准确的 opt-in 描述（如 "watches for the coding agent, optionally auto-answers the codex trust prompt when `agent.auto_trust` is enabled (codex-only, within the startup window), and pulls focus to the new workspace"）

## 3. 验证

- [x] 3.1 `cargo test` 全绿（含改名的默认值测试与未改动的 verbatim 透传/auto_trust/unknown-key 测试）
- [x] 3.2 `cargo build --release` 成功
- [x] 3.3 grep 完成判据：`grep -rn "nathanflurry" README.md src/ herdr-plugin.toml` 零命中；`grep -n "start_codex" src/main.rs` 零命中
- [x] 3.4 codex 引用复核：确认 `src/main.rs` 的 `label == "codex"` 门控、auto_trust doc comments、README "Agent startup handling" 的 auto_trust 段落、`openspec/specs/` 现行 spec 的 codex 引用全部未被误改（仍为 codex-only 行为的事实描述）；`openspec/changes/archive/**` 未被触碰
- [x] 3.5 `openspec validate --change agent-agnostic-identity`（或按 CLI 提示）通过
