# Delta Spec: plugin-config

## MODIFIED Requirements

### Requirement: 配置文件唯一定位于插件配置目录

插件 SHALL 仅从 `$HERDR_PLUGIN_CONFIG_DIR/config.toml` 读取用户配置（路径由 herdr 注入的环境变量解析，MUST NOT 硬编码绝对路径）。配置文件不存在时，插件 SHALL 以全部内置默认值运行。插件 MUST NOT 将进程环境变量或 `.env` 文件作为配置来源。

#### Scenario: 配置文件不存在时使用默认值

- **WHEN** `$HERDR_PLUGIN_CONFIG_DIR/config.toml` 不存在
- **THEN** 插件以全部内置默认值正常运行（`jj.base_rev = "trunk()"`、`jj.workspace_root = "~/.herdr/workspaces"`、`agent.command = "codex"`、`agent.bootstrap_paths` 为 34 项跨 agent 默认清单，定义见"分节 schema 与内置默认值"）

#### Scenario: 进程环境变量不再作为配置通道

- **WHEN** 进程环境中存在 `JJ_BASE_REV=main@origin`，且 config.toml 未定义 `jj.base_rev`
- **THEN** 插件使用内置默认值 `trunk()`，环境变量被忽略

### Requirement: 分节 schema 与内置默认值

配置文件 SHALL 使用分节结构：`[jj]` 节含 `base_rev`（字符串，revset 语法）与 `workspace_root`（字符串，支持 `~` 展开）；`[agent]` 节含 `command`（字符串）与 `bootstrap_paths`（字符串数组）。各键内置默认值 SHALL 为：`base_rev = "trunk()"`、`workspace_root = "~/.herdr/workspaces"`、`command = "codex"`、`bootstrap_paths` 为下列 34 项（顺序即传入 `jj sparse set` 的顺序）：

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

### Requirement: bootstrap_paths 为跨 agent 共用的单一列表

`agent.bootstrap_paths` SHALL 为单一字符串数组，供所有 agent 类型共用：创建 jj 工作区时以 `jj sparse set --clear --add <path>...` 逐条传入。数组中不存在于仓库的路径 MUST NOT 导致错误或警告（由 jj sparse 机制静默跳过）。内置默认值 SHALL 仅覆盖各 agent **启动时同步读取**的仓库级路径：右侧 pane 在启动数秒后执行全量物化（`sparse set --clear --add .`），此后发生的懒加载 / JIT 读取（子目录 `AGENTS.md`、按 frontmatter 触发的规则等）天然被覆盖，无需 bootstrap 条目。

#### Scenario: 引导文件物化

- **WHEN** 用户通过 wizard 创建 jj 工作区
- **THEN** 新工作副本的 sparse 模式被设为仅含 `bootstrap_paths` 列出的路径，对应存在的文件被物化

#### Scenario: 不存在的引导路径静默跳过

- **WHEN** `agent.bootstrap_paths` 含仓库中不存在的路径（如 `.codex`）
- **THEN** workspace 创建成功、无报错，该路径仅作为 sparse 模式常驻

#### Scenario: 跨 agent 默认覆盖

- **WHEN** 仓库同时含有 `CLAUDE.md` 与 `.cursor/rules/`，用户未配置 `bootstrap_paths`
- **THEN** 两者均在启动阶段物化（默认清单同时含 `CLAUDE.md` 与 `.cursor`），无论 `agent.command` 配置为哪个 agent
