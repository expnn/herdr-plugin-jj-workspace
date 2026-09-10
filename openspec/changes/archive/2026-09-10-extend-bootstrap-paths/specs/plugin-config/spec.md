# Delta Spec: plugin-config

## MODIFIED Requirements

### Requirement: 分节 schema 与内置默认值

配置文件 SHALL 使用分节结构：`[jj]` 节含 `base_rev`（字符串，revset 语法）与 `workspace_root`（字符串，支持 `~` 展开）；`[agent]` 节含 `command`（字符串）、`bootstrap_paths`（字符串数组）与 `extend_bootstrap_paths`（字符串数组）。各键内置默认值 SHALL 为：`base_rev = "trunk()"`、`workspace_root = "~/.herdr/workspaces"`、`command = "codex"`、`bootstrap_paths` 为下列 34 项（顺序即传入 `jj sparse set` 的顺序）、`extend_bootstrap_paths = []`：

- 根目录指令文件（8）：`AGENTS.md`、`AGENT.md`、`AGENTS.override.md`、`CLAUDE.md`、`CLAUDE.local.md`、`GEMINI.md`、`QWEN.md`、`CRUSH.md`
- 根目录工具专属文件（8）：`.mcp.json`、`opencode.json`、`opencode.jsonc`、`.cursorrules`、`.windsurfrules`、`.goosehints`、`.augment-guidelines`、`.github/copilot-instructions.md`
- 目录（18）：`.agents`、`.claude`、`.codex`、`.cursor`、`.gemini`、`.qwen`、`.opencode`、`.windsurf`、`.devin`、`.clinerules`、`.cline`、`.kilo`、`.kilocode`、`.augment`、`.continue`、`.github/instructions`、`.crush`、`.goose`

默认清单 SHALL 为对上一版本 4 项默认（`AGENTS.md`、`AGENTS.override.md`、`.codex`、`.agents`）的纯超集。清单的调研依据（15 个主流编码 agent 的官方文档矩阵与排除理由）记录于 change `agent-agnostic-bootstrap-paths` 的 design.md。

#### Scenario: 显式配置覆盖默认值

- **WHEN** config.toml 定义 `[jj] base_rev = "dev"` 与 `[agent] command = "opencode"`
- **THEN** 新工作区基于 revset `dev` 创建；左 pane 以 `opencode` 启动 agent

#### Scenario: workspace_root 支持 ~ 展开

- **WHEN** config.toml 定义 `jj.workspace_root = "~/code/workspaces"`
- **THEN** 新工作区检出目录解析到用户 home 下的 `~/code/workspaces/...`

## ADDED Requirements

### Requirement: extend_bootstrap_paths 为加法扩展

`agent.extend_bootstrap_paths` SHALL 为字符串数组（默认空），其条目 SHALL 追加到解析后的 `agent.bootstrap_paths`（用户显式值，若有；否则 34 项内置默认）之后，形成生效的物化清单，追加时 SHALL 按序去重。`extend_bootstrap_paths` 未设置时行为与现状逐字节一致。显式 `bootstrap_paths = []` SHALL 表示清空基线（与缺席键使用默认的行为不同）；`[]` 与 extend 联用即白名单模式。extend 条目 SHALL 与 `bootstrap_paths` 条目遵守同一校验：空字符串条目 MUST 拒绝并定位到 `agent.extend_bootstrap_paths[i]`；路径为 repo 相对（`~` 不展开）。本 change 不提供减法机制（见 design.md D3），不改变 `bootstrap_paths` 的整体替换语义。

#### Scenario: 在默认上扩展

- **WHEN** 用户未配置 `bootstrap_paths`，配置 `extend_bootstrap_paths = ["docs/AGENTS.md"]`
- **THEN** 生效清单为 34 项默认 + `docs/AGENTS.md`

#### Scenario: 在自定义基线上扩展

- **WHEN** 用户配置 `bootstrap_paths = ["AGENTS.md"]` 与 `extend_bootstrap_paths = ["docs/X.md"]`
- **THEN** 生效清单为两项（永远叠加，不因自定义基线而失效）

#### Scenario: 显式空数组清空基线

- **WHEN** 用户配置 `bootstrap_paths = []` 且未配置 extend
- **THEN** 启动阶段不物化任何引导文件（全量物化仍由右侧 pane 完成）

#### Scenario: 白名单模式

- **WHEN** 用户配置 `bootstrap_paths = []` 与 `extend_bootstrap_paths = ["docs/AGENTS.md"]`
- **THEN** 启动阶段仅物化 `docs/AGENTS.md`

#### Scenario: 空 extend 条目被拒绝

- **WHEN** 用户配置 `extend_bootstrap_paths = ["AGENTS.md", ""]`
- **THEN** 插件拒绝运行，错误定位到 `agent.extend_bootstrap_paths[1]`

#### Scenario: 去重保序

- **WHEN** 用户配置 `extend_bootstrap_paths = ["AGENTS.md"]`（已在默认清单中）
- **THEN** 生效清单中 `AGENTS.md` 只出现一次，顺序不变
