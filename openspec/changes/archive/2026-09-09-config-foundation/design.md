# Design: config-foundation

## Context

插件现状用两层非正式机制读配置：`config_value()` 先查进程环境变量，再逐行解析插件配置目录下的隐藏文件 `.env`（`src/main.rs:1304-1319`）。进程环境随 herdr server 启动方式漂移（`herdr --remote` 自举时 PATH 精简），`.env` 隐藏且无校验。herdr 侧已源码确认（`~/Sources/ThirdParty/herdr`）：

- `HERDR_PLUGIN_CONFIG_DIR` 由 herdr 注入（`env.rs:22-24`），目录在每次插件命令/面板 spawn 前必然 `create_dir_all` 存在（`runtime.rs:34`）；
- herdr 对目录内容零读取、零校验、零清理（`plugin_paths.rs:27-44`），"插件拥有文件格式与生命周期"（plugins.mdx:267-270）；
- 插件无法自设任何 `HERDR_PLUGIN_*` 变量（`panes.rs:346-359` scrub），herdr 也没有配置 schema 机制（`manifest.rs:11-34` 无 config 节）。

因此 config.toml 是唯一合法且可靠的插件自管配置通道。

## Goals / Non-Goals

**Goals:**

- 建立唯一配置通道 `$HERDR_PLUGIN_CONFIG_DIR/config.toml`，带类型化 schema 与严格解析语义（fail-fast + 未知键硬拒绝）。
- 迁移现有三个配置项（`jj.base_rev` / `jj.workspace_root` / `agent.command`），默认值与现状逐字节一致，零行为回归。
- 新增 `agent.bootstrap_paths` 配置化（默认值 = 现硬编码 `CODEX_BOOTSTRAP_PATHS`，行为不变）。
- 为后续 change 预留结构：`jj.command`（jj-path-resolution）、`agent.auto_trust`（agent-decoupling，仅注释占位）、`jj.base_rev` 的仓库级覆盖（per-repo-base-rev）。

**Non-Goals:**

- 不做进程 env / `.env` 的兼容读取或自动迁移（用户明确要求彻底退出）。
- 不实现 `agent.auto_trust`、`jj.command` 的 PATH 回退策略、仓库级 `jj.base_rev` 覆盖（各自属于后续 change）。
- 不引入 `agent.family`（运行时 agent 类型以 herdr 检测为准）。
- 不在 `herdr-plugin.toml` 里声明配置 schema（herdr 会静默丢弃未知 manifest 节）。

## Decisions

### D1: 单通道 config.toml，进程 env 与 .env 彻底退出

**选择**：`config_value()` 重构为 `load_config() -> Config`，只读 `<HERDR_PLUGIN_CONFIG_DIR>/config.toml`，一次性加载、全量校验、进程内共享。

**理由**：进程环境随 server 启动方式漂移，把 env 放在优先级链里意味着一个启动 shell 里偶然 export 的变量会静默压过用户配置文件（不可见、难排查）。插件配置全部是持久偏好、没有 per-run 参数（运行时上下文走 `HERDR_PLUGIN_CONTEXT_JSON`），env 通道价值为零。
**替代方案**：12-factor 式 `env > file > default` —— 被否，理由如上；`file > env > default` 折中 —— 被否，调试价值趋近于零，徒增心智负担。

### D2: 分节 schema + deny_unknown_fields

**选择**：

```toml
[jj]
base_rev = "trunk()"                      # String (revset)
workspace_root = "~/.herdr/workspaces"    # String (path, ~ 展开)
# command —— 本 change 不实现，jj-path-resolution change 定义

[agent]
command = "codex"                         # String
bootstrap_paths = ["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]  # Vec<String>
# auto_trust —— 仅注释占位，agent-decoupling change 定义
```

结构体带 `#[serde(deny_unknown_fields)]`，文件不存在 = 全默认。

**理由**：分节与领域边界（jj 操作参数 / agent 启动行为）同构，且后续仓库级覆盖文件可复用同一 schema 的 `[jj]` 子集。`deny_unknown_fields` 让手误（`bas_rev`）立即暴露；代价是用新版配置跑旧版插件会报错——边缘场景，可接受。
**替代方案**：扁平键 —— 被否，键数增长后可读性差，仓库级文件复用别扭。

### D3: fail-fast 解析语义

**选择**：语法错误 / 类型错误 / 空字符串值 → 统一在 `load_config()` 报错：
- `wizard` 入口：在 TUI 中渲染错误信息后退出；
- `open` / `remove` 等 headless action：stderr + 非零退出码，且插件自发 `herdr notification show` toast（验收证伪了"herdr 会代为呈现 action 失败"的假设——action 的 stderr 没有任何可见出口，插件必须自己弹 toast）。toast 平台限制（实测 + herdr 源码核实）：body ≤ 240 字符且单行渲染、不可复制、时长按 kind 硬编码（插件通知 ~3s）——因此 toast 仅承载首行摘要（~120 字符）+ 完整日志指引；完整错误带 UTC 时间戳追加到 `$HERDR_PLUGIN_STATE_DIR/error.log`（`~/.local/state/herdr/plugins/<id>/error.log`，持久累积、可复制可 grep）。日志写入失败不遮蔽原错误（best-effort）；
- 空字符串（如 `agent.command = ""`）视为无效配置，**不再**沿用现状"空串 = 默认值"的宽容语义。

**理由**：宁可不可用也不静默用错配置；现状的静默降级正是本次要消除的故障模式。
**替代方案**：降级到默认值 + 警告 —— 被否，typo 会被掩盖。

### D4: 两个 command 键的类型不对称

**选择**：`jj.command`（#2 落地时）为 `String | Vec<String>`（string = 可执行文件，不含参数；list = 完整 argv，进程内直接 exec）；`agent.command` 仅 `String`。

**理由**：执行模型不同。`jj` 由插件进程内 `std::process::Command` 直接 exec——argv 类型自然、支持包装脚本；`agent.command` 经 `herdr pane run` **敲入 pane 的交互式 shell PTY**（herdr `cli/pane.rs:1047-1060` → `PaneSendInput{text, Enter}`，已源码验证），本质是一行 shell 命令——别名/函数/`$VAR` 展开免费可用（herdr pane spawn 无参数、`isatty` → 加载 rc 文件）。list 形式对 agent.command 要么退化为字符串要么破坏 shell 语义。
**文档义务**：README 写明 `agent.command` 支持别名及其机制前提。

### D5: bootstrap_paths 单列表跨 agent 共用

**选择**：`Vec<String>`，默认 `["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]`，写入 `jj sparse set --clear --add ...` 时逐条传递。

**依据**：实验验证（jj 0.45.1 真实仓库）：sparse 模式是 fileset 表达式，`--add` 不校验匹配——不存在的路径退出码 0、物化 0 文件。因此无需按 agent 分列表。
**已知副作用（写入 README）**：不存在的模式常驻 `jj sparse list`；若仓库将来出现同名文件会自动物化。

### D6: 路径展开

**选择**：`jj.workspace_root` 做 `~` 展开（`dirs`/`shellexpand` 既有能力或手写首字符 `~` 替换）；`agent.bootstrap_paths` 为仓库相对路径，不做展开。

## Risks / Trade-offs

- [存量 .env 用户静默丢配置] → 用户明确接受（"不关心兼容性"）；README 迁移说明兜底。
- [deny_unknown_fields 导致新版配置跑旧版插件报错] → 错误信息会指出未知键名，用户可自行定位；版本锁定场景可接受。
- [fail-fast 让一次 typo 阻塞创建 workspace] → 这是特性而非缺陷：错误信息直指键名与行号，优于静默错配置。
- [空串语义变化] → 从"当默认值"变为报错，属 **BREAKING**，已在 proposal 标注。
- [config.toml 写一半时插件启动读到截断文件] → 解析报错走 fail-fast，行为明确（比 .env 逐行解析读到半行更好）。

## Migration Plan

1. 本 change 落地后：删除 `.env` 读取路径与 `.env.example`，README 配置节重写为 config.toml（含从 .env 迁移的手动对照表）。
2. 回滚策略：单 commit revert 即可回到 .env 行为（无数据迁移、无持久状态）。

## Open Questions

（无 —— 全部决策点已与用户定案：单通道、分节、fail-fast、硬拒绝、bootstrap_paths 归 #1、`jj.workspace_root` 命名。）
