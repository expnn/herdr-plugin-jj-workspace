# Proposal: agent-agnostic-bootstrap-paths

## Why

`agent-decoupling` 已把插件的运行时行为与 Codex 解耦（就绪状态机、运行时标签 toast、可配置 `agent.command`），但 `agent.bootstrap_paths` 的内置默认值仍是 Codex 视角的 4 项（`AGENTS.md`、`AGENTS.override.md`、`.codex`、`.agents`）。使用 claude / opencode / gemini / cursor 等 agent 的用户，其新工作区在启动窗口期内得不到任何引导文件物化——sparse checkout 要等右侧 pane 数秒后全量物化才有内容。

本次变更基于对 15 个主流编码 agent 官方文档的调研（2026-09-10，5 路并行调研，全部一级来源，矩阵见 design.md），把默认值更新为跨 agent 完整并集。

## What Changes

- `DEFAULT_BOOTSTRAP_PATHS` 从 4 项扩至 **34 项**（纯超集，旧 4 项全部保留）：
  - 根目录指令文件（8）：`AGENTS.md`、`AGENT.md`、`AGENTS.override.md`、`CLAUDE.md`、`CLAUDE.local.md`、`GEMINI.md`、`QWEN.md`、`CRUSH.md`
  - 根目录工具专属文件（8）：`.mcp.json`、`opencode.json`、`opencode.jsonc`、`.cursorrules`、`.windsurfrules`、`.goosehints`、`.augment-guidelines`、`.github/copilot-instructions.md`
  - 目录（18）：`.agents`、`.claude`、`.codex`、`.cursor`、`.gemini`、`.qwen`、`.opencode`、`.windsurf`、`.devin`、`.clinerules`、`.cline`、`.kilo`、`.kilocode`、`.augment`、`.continue`、`.github/instructions`、`.crush`、`.goose`
- README：Configuration 示例与 `agent.bootstrap_paths` 键说明更新（默认值覆盖范围 + 懒加载豁免说明）；Quickstart 中 "materializes only Codex's startup instructions" 措辞更新。
- spec：`plugin-config` 中两处默认值清单 MODIFIED，`bootstrap_paths` requirement 增加默认覆盖面 scenario 与"仅启动同步读取"的范围界定。

## Impact

- Affected specs: `plugin-config`（MODIFIED：`配置文件唯一定位于插件配置目录`、`分节 schema 与内置默认值`、`bootstrap_paths 为跨 agent 共用的单一列表`）
- Affected code: `src/main.rs` 的 `DEFAULT_BOOTSTRAP_PATHS` 常量与默认值 pin 测试；`README.md` Configuration / Quickstart 节
- 无行为 breaking：纯超集；不存在的路径由 jj sparse 静默跳过（既有语义，jj 0.45.1 实验验证）；物化顺序与右侧 pane 全量物化机制不变
- 范围外：Roo Code（2026-05-15 官方关停）与 Aider（不自动读任何指令文件）的相关路径；各家 ignore 文件；子目录级懒加载文件；全局（`~/`）路径——理由见 design.md D3
