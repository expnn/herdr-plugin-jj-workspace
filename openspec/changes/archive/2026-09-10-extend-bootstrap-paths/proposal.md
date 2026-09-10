# Proposal: extend-bootstrap-paths

## Why

`agent-agnostic-bootstrap-paths` 把 `agent.bootstrap_paths` 默认值扩至 34 项后，默认值本身变成了一堵墙：用户想加一个自家路径（如 `docs/AGENTS.md`），今天唯一的办法是把 34 项全抄进 `config.toml` 再追加——抄出来的快照还会与未来的默认演进永久漂移（默认加到 35、36 项时，快照用户感知不到）。

本次变更引入加法扩展键，让用户只声明 delta，默认演进自动继承。

## What Changes

- 新增 `[agent] extend_bootstrap_paths`（字符串数组，默认 `[]`）：条目追加到解析后的 `bootstrap_paths`（用户显式值，若有；否则 34 项内置默认）之后，追加时按序去重；未设置时行为与现状逐字节一致。
- 明确空数组语义：显式 `bootstrap_paths = []` 表示清空基线（与缺席键使用默认不同）；`[]` + extend 即白名单模式（启动阶段仅物化 extend 列出的文件）。
- 校验与现有键同规则：extend 条目中的空字符串 MUST 拒绝并定位到 `agent.extend_bootstrap_paths[i]`；路径为 repo 相对（`~` 不展开）。
- README：配置示例改为一行可抄的 extend 示例；键说明补"95% 用户只用 extend，仅完全接管时才手写 bootstrap_paths"的文档口径。
- 测试：显式 `[]` → 空的 pin 测试（刻画现有行为，实现前后都应绿）、extend-on-default、extend-on-custom、空 extend 条目拒绝、去重保序。

## Impact

- Affected specs: `plugin-config`（MODIFIED：`分节 schema 与内置默认值` 增键；ADDED：`extend_bootstrap_paths` 语义 requirement + 6 个 scenario）
- Affected code: `src/main.rs` —— `AgentConfig` 新增字段、生消清单计算（base ++ extend + 去重）与校验；`README.md` Configuration 节
- 无行为 breaking：纯加法键；未设置 extend 的旧配置解析结果逐字节不变（`deny_unknown_fields` 下新键为已知键，老文件照常通过）；34 项默认 pin 测试不动
- 范围外：减法机制（`exclude_…`，见 design.md D3）、`Vec<Cow>` 类型改造（已否决，见 design.md D5）、wizard 改动（wizard 本来就不碰 agent 配置键）
