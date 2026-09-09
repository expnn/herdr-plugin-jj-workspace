# Proposal: config-foundation

## Why

插件当前用进程环境变量 + 隐藏的 `.env` 文件管理配置，存在三个问题：`.env` 是隐藏文件、用户不易发现；进程环境随 herdr server 的启动方式漂移（如 `herdr --remote` 自举时 PATH 精简），作为配置通道不可靠且可能静默覆盖用户意图；配置无 schema、无校验，写错键名会被静默忽略。后续三个 change（jj 路径解析、agent 解耦、仓库级 base_rev）都需要引入新配置键，必须先立起一个干净、可校验的配置基座。

## What Changes

- **BREAKING**：配置通道收敛为唯一文件 `$HERDR_PLUGIN_CONFIG_DIR/config.toml`（herdr 保证该目录在插件运行前必然存在，且对其内容零干涉——已源码验证）。
- **BREAKING**：进程环境变量与 `.env` 文件彻底退出配置通道，不做兼容读取；删除 `.env.example`。
- 新增分节 TOML schema（`[jj]` / `[agent]`），启用 `deny_unknown_fields` 硬拒绝未知键。
- 解析失败（语法错误、类型错误、空字符串值）时 fail-fast：wizard 界面显示错误、action 以非零码退出并 toast 提示。
- 现有配置项迁移换名（行为不变）：`JJ_BASE_REV` → `jj.base_rev`、`JJ_WORKSPACE_ROOT` → `jj.workspace_root`、`JJ_START_COMMAND` → `agent.command`。
- 新增 `agent.bootstrap_paths` 配置项：单一字符串数组、跨 agent 共用；默认值 = 现硬编码的 Codex 引导文件列表，行为不变。不存在的路径由 jj sparse 机制静默跳过（jj 0.45.1 实验验证）。
- `agent.command` 保持"敲入 pane 交互式 shell"的注入模型，支持别名/函数/shell 语法；文档写明该机制前提。
- `jj.command` 不在本 change 范围（由 jj-path-resolution change 定义）；`agent.auto_trust` 仅在配置文件注释中占位，不实现（由 agent-decoupling change 定义）。
- README 配置章节重写为 config.toml 形式。

## Capabilities

### New Capabilities

- `plugin-config`: 插件配置文件的定位、格式、解析语义（fail-fast、未知键硬拒绝）、键清单与默认值、各键的执行模型（argv 直接执行 vs shell 行注入）。

### Modified Capabilities

（无 —— 仓库尚无主 specs，本 change 是首个能力 spec。）

## Impact

- `src/main.rs`：`config_value()` 整体重构为 config.toml 解析层；`resolve_start_command()`、`cmd_wizard`、`cmd_remove`、右侧 pane setup 命令生成处的配置读取全部改走新解析层。
- `Cargo.toml`：新增 `toml` / `serde` 依赖（serde 已存在则仅加 derive 场景确认）。
- 删除 `.env.example`；README 配置章节重写。
- 运行时行为不回归：所有键的内置默认值与现状一致（`trunk()` / `~/.herdr/workspaces` / `codex` / 现硬编码 bootstrap 列表）。
- 明确不受影响：herdr 注入的运行时环境变量（`HERDR_PLUGIN_CONFIG_DIR`、`HERDR_PLUGIN_CONTEXT_JSON`、`HERDR_BIN_PATH` 等）——它们是运行时上下文，不是用户配置。
