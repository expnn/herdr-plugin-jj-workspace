# Proposal: wizard-single-source

## Why

wizard 的 source 候选列表按 herdr workspace 聚合、且只看 `active_tab` 的 focused/首个 pane（`load_workspace_choices_with`），同一 herdr workspace 最多贡献一条候选。focus workspace 的**其他 tab** 即使是 jj 仓库也永远不可见，而非 focus workspace 的 pane 反而能进列表。用户最符合直觉的期望——"为我所在的 pane 的仓库建一个 jj workspace"——会被违背：focus pane 非 jj（但同 workspace 另有 jj pane）时，focus 的 workspace 反而没有任何目录进入候选。列表机制已无存在价值：用户真正想要的永远是当前激活 pane 的仓库，做减法。

## What Changes

- **删除 source 候选列表**：`Source Workspace` 从可搜索列表变为只读展示 section（显示消解后的主仓库根），section 顺序调整为 New Workspace Name → Base（jj revset）→ Source Workspace → Checkout；Source/Checkout 不进 Tab 焦点循环（Name ↔ Base 循环）。
- **单一源 = 调用者聚焦 pane 的目录**：`HERDR_PLUGIN_CONTEXT_JSON.focused_pane_cwd`（pane 的 shell cwd，非 `foreground_cwd`）经 `jj_root()` 上溯到 jj workspace 根，再经 `repo_root()` 消解副 workspace 到主仓库根（复用现有 helper）。
- **源获取走 herdr 插件上下文注入，零 herdr CLI 回调**：action 进程（`cmd_open`）与 overlay pane 进程（wizard）各自被注入调用者 context（herdr 服务端在 open 时构造，`panes.rs` overlay 路径用 `current_plugin_context`；`HERDR_PLUGIN_CONTEXT_JSON` 为受保护 key 不可伪造）。现有 `herdr workspace list` + `herdr pane list` 两次拉取与全量过滤整体删除；`--env CURRENT_HERDR_WORKSPACE_ID` 自定义转发删除（`workspace_id` 同样从自身 context 取）。
- **预检与报错分层**：`cmd_open`（headless action）解析自身 context 即做 jj 预检——非 jj / 无聚焦 pane 目录时 `die()` toast 报错并退出（action stderr 无可见出口，toast 是唯一通道），不开 wizard pane；wizard 入口保留 fail-fast TUI modal 兜底（防绕过 action 直开 pane entrypoint）。
- **cwd 语义收敛**：源目录语义固定为 `pane.cwd`（shell OSC 7 上报目录，herdr label/follow-cwd 同款权威口径），不再优先 `foreground_cwd`（其实时进程目录曾导致 pyright dist 类仓库外深目录隐患）。
- **BREAKING**：wizard 不再列出或允许选择其他 herdr workspace 的仓库；同一 workspace 内非聚焦 tab 的 jj 仓库不再可选（需先切到对应 pane 再触发 action）。"在非 jj 目录里打开同文件夹"的 fallback 早已随 source-jj-only 删除，本 change 不恢复它。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `workspace-wizard`：source 由"jj-only 候选列表"变为"调用者聚焦 pane 消解的单一主仓库根，只读展示"；section 顺序、焦点循环、空态/过滤需求、base 预填两阶段逻辑随之变更。需 delta spec。

## Impact

- `src/main.rs`：删除 `load_workspace_choices*`、`herdr_json_with`、`WorkspaceChoice`、`pane_path`、模糊过滤与上下导航、`WizardField::WorkspaceSearch` 及 query/filtered/selected 状态、`--env CURRENT_HERDR_WORKSPACE_ID` 转发；`cmd_open` 新增自身 context 读取 + `jj_root`/`repo_root` 预检 + `die()`；`run_workspace_wizard` 状态机与 `draw_workspace_wizard` section 顺序重写；Tab 循环收敛为 Name ↔ Base；`wizard_final_base_rev` 的"按最终选中重算"注释与逻辑简化（源固定）；相关测试（假 herdr fixture、候选过滤、空态、fuzzy）删除并新增单源用例。
- `openspec/specs/workspace-wizard/spec.md`：候选/空态/过滤/占位类 Requirement 重写（见 Capabilities）。
- README：wizard 流程段（source selector 描述）同步更新。
- 依赖：独立 change；不新增配置键（`config.toml` 唯一通道原则不变）；未发布故不 bump version。
