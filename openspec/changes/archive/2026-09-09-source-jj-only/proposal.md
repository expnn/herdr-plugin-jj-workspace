# Proposal: source-jj-only

## Why

插件的身份是 "New **jj workspace**"（`herdr-plugin.toml` description：*Create jj workspaces with Codex on the left and a terminal on the right*），但 wizard 的候选列表来自 `herdr workspace list` 且不做 jj 过滤，导致非 jj 项目也进入选择器，并在选中后走"打开同文件夹"的 fallback 支路。非 jj 目录不存在"创建 jj workspace"的语义——该 fallback 只是继承自 herdr worktree modal 的血统，不是本插件的职责。它迫使 wizard 承载两套语义：非 jj 源的置灰态、黄色 warning、`[dir]` badge、`folder` 预览标签、双份右 pane 启动路径与 toast，全部为一份不存在的功能服务。

## What Changes

- **候选源过滤 jj-only**：`load_workspace_choices` 的结果只保留含 `.jj` 目录的 herdr workspace（候选路径统一为 workspace 根，回退路径祖先归一），非 jj 项目不再出现在 wizard 列表中。
- **删除整个非 jj 支路**：
  - `source_warning`（"not a jj workspace" 黄色警告）整体删除；
  - wizard 渲染中的 `[dir]` badge、`folder` 预览标签（`checkout/folder/workspace` 三态变体）、base/name 字段的非 jj 置灰逻辑删除；列表项恒为 jj 仓库；
  - Enter 提交路径只保留 jj 分支：`run_workspace_wizard` Enter 处理里 `is_jj_workspace(&source.path)` 条件（main.rs:1480、1488）与其非 jj else 支路（返回输入 base 原值）删除，base 校验恒执行；
  - `cmd_wizard` 内 `is_jj_workspace(&source)` 判定与 non-jj 目的路径分支（main.rs:692-750）删除，恒走 `jj workspace add` + bootstrap + setup 脚本路径；
  - `open_tab_layout` 的 `is_jj=false` 路径删除：右 pane 恒走 `setup_script_command`；`finish_tab_shell_command`（右 pane 直跑 finish-tab）成为 dead code 并删除——`cmd_finish_tab` 本体保留（setup 脚本仍经 exe 自定位后台启动它）；
  - "No jj workspace created" toast 删除。
- **BREAKING**：wizard 不再提供"在非 jj 项目里打开同文件夹"的能力（该场景不依赖插件 fallback，由 herdr 原生覆盖）。
- **spec 收敛**：`workspace-wizard` 的"非 jj 源置灰"Requirement 与相关场景删除，替换为"候选源仅限 jj workspace"的需求。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `workspace-wizard`: 源候选列表仅保留含 `.jj` 目录的 herdr workspace；删除非 jj 源的置灰渲染、folder 打开语义与相关场景。

## Impact

- `src/main.rs`：`load_workspace_choices`（候选路径统一根 + jj 过滤）、`source_warning`（删）、`draw_workspace_wizard`（去 badge/置灰/folder 预览、空态分支）、Enter 提交分支（只留 jj 路径）、`cmd_open`/`cmd_wizard`（移除 `JJ_CURRENT_CWD` 采集与传递）、`open_tab_layout`（删 `is_jj=false` 路径与参数）、`finish_tab_shell_command`（删，dead code）、`is_jj_workspace`（删，无引用）。
- `openspec/specs/workspace-wizard/spec.md`：需求增删（见 Capabilities）。
- README：wizard 流程段删除非 jj fallback 描述（"If the selected folder is not a jj workspace…"），source selector 说明更新为 jj-only。
- 依赖：独立于 UI 美化 change（后者待本 change 落地后按纯 jj 世界设计）。
