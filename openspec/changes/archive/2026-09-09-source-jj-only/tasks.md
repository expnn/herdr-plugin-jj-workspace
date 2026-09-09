# Tasks: source-jj-only

## 1. 候选路径统一根与 jj 过滤（D1）

- [x] 1.1 删除 `load_workspace_choices` 中 `current_cwd` 对当前 workspace 的路径覆盖（main.rs:847-848）；候选路径统一为 `checkout_path`（workspace 根）。
- [x] 1.2 checkout_path 缺失的回退路径（main.rs:855-868）实现祖先归一：从 pane cwd 向上找 `.jj`，找到则以 `.jj` 所在目录为候选路径，找不到即非 jj → 过滤。
- [x] 1.3 `cmd_open`（main.rs:588）移除 `JJ_CURRENT_CWD` env 传递与 `focused_pane_cwd` 采集；`cmd_wizard` 对应读取删除。
- [x] 1.4 删除 `is_jj_workspace`（main.rs:2066）——6 处调用（661/692/1480/1488/1734/1799/1802）全部随本 change 消失后函数无引用；`cmd_remove` 的内联 `.jj` 存在性判断（main.rs:1317）不受影响。
- [x] 1.5 单元测试：fake herdr 提供 checkout_path 全覆盖/缺失/非 jj 三种 workspace list，断言候选仅含 jj 根路径、子目录被归一、非 jj 被过滤（复用现有 `make_fake_herdr` 模式或独立谓词测试）。

## 2. 空候选打开 wizard 并显示空态（D2）

- [x] 2.1 `cmd_wizard`（main.rs:645-649）删除 `Ok(_) => fail("Herdr has no workspaces to select")`，候选为空时照常进入 `run_workspace_wizard`。
- [x] 2.2 `draw_workspace_wizard` 新增空态分支：`choices.is_empty()` 时不 return（main.rs:1689-1691），在 source 列表区域渲染"no jj workspaces"空态提示（基础一行样式，文案如 `no jj workspaces — open herdr's project picker instead`）。
- [x] 2.3 空态下 Enter 不产生创建（现有 `filtered.get(selected)` None → error 路径复用），esc 正常退出；确认无 panic。

## 3. wizard 渲染移除 kind 维度（D4）

- [x] 3.1 删除 `source_warning`（main.rs:86-92）及其在 `draw_workspace_wizard` 的 warning 渲染（main.rs:1801-1814、1855-1861），错误行只承载 error。
- [x] 3.2 删除列表项 `[dir]`/`[jj]` badge 与 `kind` 变量（main.rs:1734-1751），候选恒为 jj。
- [x] 3.3 checkout 预览固定为 jj 目的路径，删除 `folder`/`workspace` 标签变体（main.rs:1801-1815）；`selected_is_jj` 相关条件（main.rs:1797-1800、1835-1839）随之收敛。
- [x] 3.4 删除/更新依赖被删符号的测试：`non_jj_sources_warn_that_base_rev_is_ignored`（main.rs:2930）。

## 4. 提交路径收窄

- [x] 4.1 `run_workspace_wizard` Enter 处理：删除 `is_jj_workspace(&source.path)` 条件（main.rs:1480 checkout 存在性检查、main.rs:1488 base 求解）及非 jj else 支路（返回输入 base 原值），base 校验恒执行（dirty 惰性语义不变）。
- [x] 4.2 `cmd_wizard` 预填：`initial_base` 的 `is_jj_workspace(&initial_choice.path)` 条件（main.rs:661-672）删除，恒走 `resolve_base_rev`（候选已保证 jj）。
- [x] 4.3 `cmd_wizard` 主流程：删除 `is_jj = is_jj_workspace(&source)`（main.rs:692）与 non-jj 目的路径分支（main.rs:748-750），恒走 `jj workspace add` + bootstrap + setup 脚本路径；`fail("workspace folder does not exist")` 守卫保留。

## 5. open_tab_layout 收窄（D3）

- [x] 5.1 `open_tab_layout`（main.rs:918）删除 `is_jj` 参数与 `is_jj=false` 分支（右 pane 恒 `setup_script_command`）；调用点（main.rs:752-760）同步。
- [x] 5.2 删除 `finish_tab_shell_command`（main.rs:1175，dead code）与 "No jj workspace created" toast（main.rs:990-1010）。
- [x] 5.3 确认 `cmd_finish_tab`（main.rs:1191）与 setup 脚本后台启动路径不受影响（脚本经 exe 自定位启动 finish-tab，保持不变）。

## 6. 验证与文档同步

- [x] 6.1 spike 验证真实 `herdr workspace list` 中 checkout_path 覆盖率与回退归一正确性（见 design Risks）。
- [x] 6.2 README：删除非 jj fallback 描述（"If the selected folder is not a jj workspace…" 段，README.md:58-60），source selector 说明更新为"候选仅限 jj 仓库、新 tab 恒建在 workspace 根"。
- [x] 6.3 运行 `cargo build --release` 与 `cargo test` 全绿（含新增过滤/归一/空态/渲染收敛测试）；有 TTY 时手动冒烟：jj 仓库与纯目录混合的 workspace 列表，确认非 jj 项不可见、子目录启动建根、全非 jj 时空态显示。
- [x] 6.4 `openspec validate --change source-jj-only` 通过；归档时 main spec 同步（workspace-wizard 新增/修改/删除需求）。