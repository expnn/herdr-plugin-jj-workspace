# plugin-config Delta

## ADDED Requirements

（无）

## MODIFIED Requirements

### Requirement: 配置文件唯一定位于插件配置目录
插件 SHALL 仅从 `$HERDR_PLUGIN_CONFIG_DIR/config.toml` 读取用户配置（路径由 herdr 注入的环境变量解析，MUST NOT 硬编码绝对路径）。配置文件不存在时，插件 SHALL 以全部内置默认值运行。插件 MUST NOT 将进程环境变量或 `.env` 文件作为配置来源。

#### Scenario: 配置文件不存在时使用默认值
- **WHEN** `$HERDR_PLUGIN_CONFIG_DIR/config.toml` 不存在
- **THEN** 插件以全部内置默认值正常运行（`jj.base_rev = "trunk()"`、`jj.workspace_root = "~/.herdr/workspaces"`、`agent.command = "opencode"`、`agent.bootstrap_paths = ["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]`）

#### Scenario: 进程环境变量不再作为配置通道
- **WHEN** 进程环境中存在 `JJ_BASE_REV=main@origin`，且 config.toml 未定义 `jj.base_rev`
- **THEN** 插件使用内置默认值 `trunk()`，环境变量被忽略

### Requirement: 分节 schema 与内置默认值
配置文件 SHALL 使用分节结构：`[jj]` 节含 `base_rev`（字符串，revset 语法）与 `workspace_root`（字符串，支持 `~` 展开）；`[agent]` 节含 `command`（字符串）与 `bootstrap_paths`（字符串数组）。内置默认值：`base_rev = "trunk()"`、`workspace_root = "~/.herdr/workspaces"`、`command = "opencode"`、`bootstrap_paths = ["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]`。默认 `command` 仅是开箱即用的便利值，不是能力边界：插件对 herdr 能检测的任意 agent 均可用（`agent.command` 可设为任意经用户 shell 解析的命令）。

#### Scenario: agent.command 默认值
- **WHEN** config.toml 中未定义 `agent.command`
- **THEN** 新工作区左 pane 以 `"opencode"` 启动 agent

#### Scenario: 显式配置覆盖默认值
- **WHEN** config.toml 定义 `[jj] base_rev = "dev"` 与 `[agent] command = "codex"`
- **THEN** 新工作区基于 revset `dev` 创建；左 pane 以 `codex` 启动 agent

#### Scenario: workspace_root 支持 ~ 展开
- **WHEN** config.toml 定义 `jj.workspace_root = "~/code/workspaces"`
- **THEN** 新工作区检出目录解析到用户 home 下的 `~/code/workspaces/...`

## REMOVED Requirements

（无）
