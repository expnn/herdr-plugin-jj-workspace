# Design: agent-agnostic-identity

## Context

agent-decoupling（2026-09-09 归档）删除了全部 Codex 硬编码逻辑（文案爬屏、`agent=="codex"` 过滤、硬编码 toast），运行时已是 agent 无关。但插件对外的身份声明层未跟进：manifest description、README 开篇、setup 脚本头部注释、代码注释与一个变量名仍自称 Codex。同时作者改名（nathanflurry → expnn，git 历史与 LICENSE 署名佐证）留下 id 漂移：manifest id 为 `expnn.jj-workspace`，而 README 5 处与 `plugin_id()` fallback（`src/main.rs:2101`）仍为 `nathanflurry.jj-workspace`。

全部相关改动从未发布，因此默认值变更不构成存量用户迁移问题，也不做 version bump。

## Goals / Non-Goals

**Goals:**

- 插件对外身份（manifest description、README 门面）真实反映 agent 无关能力。
- 默认 `agent.command` 改为 `"opencode"`（用户定案）。
- 消除 nathanflurry/expnn id 漂移（README 与代码 fallback 统一为 `expnn.jj-workspace`）。
- 遗留命名语义化（`start_codex` 变量、stale 测试名）。

**Non-Goals:**

- 不泛化 `auto_trust`（保持 codex-only + 窗口门控，行为事实与全部相关描述保留）。
- 不改 `bootstrap_paths` 默认值（`.codex` 等是跨 agent 共用列表的数据，见决策 D2）。
- 不改写 `openspec/changes/archive/**` 历史档案。
- 不做 version bump（Cargo.toml 与 herdr-plugin.toml 版本号保持 0.5.0）。
- 不触碰现行 spec 中其他 codex 引用（均为默认值/已定案行为的事实描述）。

## Decisions

### D1 默认 `agent.command = "opencode"`：仅改默认值，不引入新的配置机制

用户显式定案。落点：`AgentConfig::default()`（`src/main.rs:268`）、README 示例与键说明（"Defaults to `codex`" → "Defaults to `opencode`"）、plugin-config delta spec。备选"删除默认值、缺省时要求用户显式配置"被否——与"配置文件不存在即全默认值运行"的现行 spec 冲突，且增加首次使用摩擦。

注意连带修正：`src/main.rs:1023` 旁注释 `(default "codex")`、README:94/123 的默认值陈述，以及两个默认值断言测试（`config.agent.command` 断言、`resolve_start_command(&AgentConfig::default())` 断言）。

### D2 `bootstrap_paths` 默认值不动

`["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]` 是单一跨 agent 共用列表（config-foundation 定案：jj sparse 对不存在路径静默跳过，见约束 #139/#145）。`.codex` 是数据不是身份声明——AGENTS.md 本身已覆盖 opencode。已知不对称（默认 command 是 opencode、默认列表含 .codex）在此明确接受为设计事实，不在 README 中回避。

### D3 身份措辞：只改"自称用 Codex"的语句，保留"codex-only 行为"的描述

四类 codex 引用中只动身份声明类（proposal 列出的 7 处）；`auto_trust` codex-only 门控（`src/main.rs:1172`）及其 README/spec 描述一字不改——删了反而失实。setup 脚本注释（原 "accepts Codex's trust prompt"）双重过时（agent-specific + 描述的是 agent-decoupling 之前的无条件回车），改为准确的 opt-in 语义描述。

### D4 id 漂移：以 manifest id 为权威，全部收敛到 `expnn.jj-workspace`

manifest id 是 herdr 读取的唯一事实源；README 的 keybinding 命令、config-dir 示例、error.log 路径，以及 `plugin_id()` fallback（`src/main.rs:2101`，`HERDR_PLUGIN_ID` 未注入时用于 `herdr plugin pane open --plugin <id>`）全部改为 `expnn.jj-workspace`。fallback 值本应与 manifest 同步，此修复消除该隐性耦合的手工漂移面（不引入自动同步机制——manifest 解析在插件进程内不可得，超出本 change）。

### D5 测试命名随语义走

`start_command_defaults_to_codex_when_unset` → `command_defaults_to_opencode_when_unset`：修正两层失实（旧键名 `start_command` 已更名 `command`；默认值现为 opencode）。其余以 codex 作示例值的测试（verbatim 透传、auto_trust 门控、unknown-key 拒绝）不改——codex 是测试数据，不是断言的身份。

## Risks / Trade-offs

- [默认值变为 opencode，而 opencode 未安装在用户机器上时左 pane 启动失败] → 与现状（codex 未安装失败）同一失败模式，由 finish-tab 的 `"<label> needs attention"` toast 与 herdr agent 检测自然呈现；README 键说明已明确"set it to whatever your shell resolves"。
- [`plugin_id()` fallback 与 manifest 仍靠人工同步，未来可能再漂移] → 本 change 修复当前漂移；自动化校验（构建时比对 manifest）记为后续候选，不在本 change 范围。
- [README 多处 id 串改，可能漏改] → 完成判据含 `grep -rn "nathanflurry" README.md src/` 零命中。

## Migration Plan

无迁移：改动未发布，合并后首次发布即携带全部修正。回滚 = 常规 git revert。

## Open Questions

（无——默认值、id 收敛、测试名、不 bump 版本均已定案。）
