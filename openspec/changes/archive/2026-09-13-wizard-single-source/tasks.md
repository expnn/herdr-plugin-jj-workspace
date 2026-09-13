## 1. 源解析与预检（cmd_open / cmd_wizard 入口）

- [x] 1.1 新增 `resolve_source_from_ctx()`：读自身 `HERDR_PLUGIN_CONTEXT_JSON`（复用 `json_string_field` 取 `focused_pane_cwd` + `workspace_id`）→ `jj_root()` 上溯 → `repo_root()` 消解主根；无值/非 jj 返回结构化错误
- [x] 1.2 `cmd_open` 接入预检：非 jj/无源时 `die()` toast 报错并退出（不开 pane）；通过后 `herdr plugin pane open` 去掉 `--env CURRENT_HERDR_WORKSPACE_ID` 转发
- [x] 1.3 `cmd_wizard` 入口改读自身 context：`workspace_id`（tab 落点）+ 源消解；失败走 fail-fast TUI modal（复用 `show_config_error_and_exit` 风格），不进向导主界面
- [x] 1.4 单测：jj 主根 / 副 workspace / 子目录 / 非 jj / `focused_pane_cwd` 缺失 / 空 context 六组

## 2. 向导状态机与渲染（单源只读）

- [x] 2.1 删除 `WizardField::WorkspaceSearch`、`query/filtered/selected` 状态、`filtered_choice_indices`、模糊匹配与上下导航；Tab 循环收敛为 Name ↔ Base
- [x] 2.2 `run_workspace_wizard` 签名改为单源（`source: WorkspaceSource` 替代 `choices/initial_selection`）；删除零候选/无匹配分支；`initial_base` 按唯一源预填，`wizard_final_base_rev` 保留 dirty 语义并更新注释
- [x] 2.3 `draw_workspace_wizard` 按新顺序渲染 New Workspace Name → Base → Source Workspace（只读主根）→ Checkout；Source/Checkout 标题恒 subtext0 粗体；顶部提示行去掉 select/filter措辞
- [x] 2.4 渲染单测：section 顺序、Source 只读不进焦点循环、Tab 只在 Name/Base 间切换

## 3. 候选管线删除与收尾

- [x] 3.1 删除 `load_workspace_choices*`、`herdr_json_with`、`WorkspaceChoice`、`pane_path` 及假 herdr fixture（`make_fake_herdr_listing`）与候选/空态/fuzzy 相关测试；确认无残留引用后 `cargo build` 零警告
- [x] 3.2 `cargo test` 全绿；`cargo clippy`（如仓库有该门禁）通过
- [x] 3.3 README wizard 段同步（source selector → 单一源只读展示；注明"先切到对应 pane 再触发 action"）；`openspec validate --change wizard-single-source` 通过
