# Design: source-jj-only

## Context

插件的唯一职责是"为一个 jj 仓库创建新的 jj workspace"（`herdr-plugin.toml` description 与 action 标题均为 jj 语义）。当前 wizard 候选来自 `herdr workspace list`（`load_workspace_choices`，main.rs:817），不做 jj 过滤；选中非 jj 项目后走"打开同文件夹"fallback（渲染置灰态 + `source_warning` + `folder` 预览 + `is_jj=false` 分支 + toast）。这条非 jj 支路服务不存在的产品语义，是后续 wizard UI 美化（独立 change）的主要复杂来源。

本 change 在源头把候选收窄为 jj 仓库，并删除整条非 jj 支路，使 wizard 成为纯"新 jj workspace"三字段表单。UI 布局美化不属于本 change（另行 change）。

关键事实（代码核查）：
- `open_tab_layout`（main.rs:918）仅被 `cmd_wizard` 调用；`is_jj=false` 时右 pane 走 `finish_tab_shell_command`（右 pane 直接前台跑 finish-tab，main.rs:1175），`is_jj=true` 时走 `setup_script_command`（setup 脚本后台自定位启动 finish-tab）。jj-only 后 `finish_tab_shell_command` 变 dead code，但 `cmd_finish_tab`（main.rs:1191）仍被 setup 脚本复用，保留。
- `repo_root`（main.rs:2078）只处理"path 本身是 workspace 根"（`.jj/repo` 为目录或指针文件）；path 是仓库子目录时原样返回。
- `JJ_CURRENT_CWD`（cmd_open, main.rs:596）取 `focused_pane_cwd` 优先 —— **可能是仓库子目录**。`load_workspace_choices` 对当前 workspace 用 `current_cwd` 覆盖 `checkout_path`（main.rs:847-848）。

## Goals / Non-Goals

**Goals:**
- wizard 候选仅含 jj 仓库（成员判定须覆盖"path 在仓库子目录内"的场景，见 D1）。
- 删除全部非 jj 专属代码路径与渲染（置灰、warning、folder 预览、is_jj=false 分支、toast、dead code）。
- `workspace-wizard` spec 收敛：删"非 jj 源置灰"，新增"候选源仅含 jj workspace"，"revset 提交预校验"去掉非 jj 限定措辞。

**Non-Goals:**
- wizard 的布局/视觉美化（独立 change，本 change 落地后按其纯 jj 世界设计）。
- 为非 jj 项目提供 `jj git init`/转换——非 jj 项目由 herdr 原生打开能力覆盖，不在本插件。
- `setup-workspace.sh`、`cmd_finish_tab`、agent 就绪/自动信任语义的任何改动。
- `herdr workspace list` 与 pane 回退取路径的既有解析结构改动。

## Decisions

### D1: 候选路径统一为 workspace 根；jj 判定只在回退路径做祖先归一

**选择**：候选路径优先级 = herdr `checkout_path`（workspace 根）> active pane `foreground_cwd`（回退）。主路径直接用 checkout_path，**不**用 `current_cwd` 覆盖当前 workspace（main.rs:847-848 的覆盖逻辑删除，`JJ_CURRENT_CWD` env 与 `cmd_open` 的 `focused_pane_cwd` 采集随之移除）。回退路径（checkout_path 缺失，main.rs:855-868）向上找祖先 `.jj`：找到则以 `.jj` 所在目录为候选路径（归一到根），找不到即非 jj → 过滤。

**理由**：用户决策——coding agent 必须从项目根获取完整上下文，子目录只会造成 agent 工作目录与 pane 目录分裂，统一 workspace 根，完全忽略子目录。该决策同时消解 O1：候选路径恒为根后，`repo_root`（main.rs:2078）永远收到带 `.jj` 的根路径，无需扩展祖先解析；`workspace_destination` 分组不再嵌套子目录 basename。

**备选**：
- 保留 `current_cwd` 覆盖并做祖先解析支持子目录源：工作目录分裂、`repo_root`/dest 分组复杂化，被用户明确否决。
- 用 `jj root`/`jj workspace root` 子进程探测：语义最准但每个候选一次 spawn，加载串行 N 次进程；fs 祖先扫描无 spawn，且仅出现在 checkout_path 缺失的回退路径，量小。

**后续影响**：`is_jj_workspace`（main.rs:2066，精确根判定）在 Enter/prefill/渲染/过滤的 6 处调用全部随本 change 删除，函数本身无引用 → 一并删除（`cmd_remove` 的 main.rs:1317 用内联 `canon.join(".jj").exists()`，不受影响）；回退路径归一可复用 `repo_root` 的 `.jj/repo` 指针逻辑或独立小函数，实现时定。

### D2: 空候选折叠为空态（source + esc，Tab/Enter 短路）

**选择**：过滤后候选为空时，wizard 照常打开但**折叠为空态**：仅渲染 source 区与空态提示（`no jj workspaces — open herdr's project picker instead`）+ esc 提示；name/base/checkout 区与创建按钮 SHALL NOT 渲染。`run_workspace_wizard` 事件循环在 `choices.is_empty()` 时**忽略 Tab/BackTab 与 Enter**（不切换字段、不走 "no matching workspace" 错误路径），用户按 esc 退出。`cmd_wizard` 的 `Ok(_) => fail("Herdr has no workspaces to select")`（main.rs:647）改为照常进入 wizard。

**理由**：jj-only 后"无 jj workspace"从边缘情况变为真实高频路径（用户环境全为非 jj 项目时按 prefix+a）。弹窗即 fail 的体验差；打开空态让用户明确知道原因并自然退出。折叠 + 事件短路是必要的一对：draw 无法阻止焦点切到被隐藏的 name/base section，必须在事件层一并收敛——否则出现"焦点在看不见的字段、Enter 弹错误行"的错位。空态样式只做基础提示，视觉美化属后续 change。

### D3: `open_tab_layout` 收窄为纯 jj 路径

**选择**：删除 `is_jj` 参数与 `is_jj=false` 分支（右 pane 恒 `setup_script_command`）；删除 `finish_tab_shell_command`（main.rs:1175，dead code）与 "No jj workspace created" toast（main.rs:990-1010）；`is_jj` 判定从 `cmd_wizard` 流程中整体移除（`jj workspace add` + bootstrap + setup 脚本路径不再条件化）。`cmd_finish_tab` 保留。

### D4: wizard 渲染移除 kind 维度

**选择**：列表项删除 `[dir]`/`[jj]` badge（`kind` 变量，main.rs:1734-1751）——候选恒为 jj，badge 无信息量；checkout 预览固定为 jj 目的路径，`folder`/`workspace` 标签变体删除；`source_warning`（main.rs:86-92）与 warning 渲染整体删除，错误行只承载 error。渲染结构调整不追求视觉美化（属后续 change），仅去掉非 jj 态。

## Risks / Trade-offs

- **[回退路径子目录误判]** checkout_path 缺失时回退到 pane cwd 并向上归一，可能把"外层某处有 `.jj`、但本候选实为独立非 jj 项目"的目录判为 jj → 与 jj 自身"最近祖先即仓库边界"语义一致（jj 也会把该目录当作仓库内），且该情况仅出现在 checkout_path 缺失的边缘路径；风险低。缓解：D1 只做存在性判定，真实仓库解析仍走提交时既有链。
- **[行为破坏]** 非 jj 项目从此无法经本插件 wizard 打开；子目录 pane 中启动 wizard 也会把新 tab 建在 workspace 根而非当前子目录 → 明确 BREAKING，README 更新；若用户实际依赖，可回退（git revert）。用户已确认不依赖非 jj 场景。
- **[spec 措辞漂移]** "revset 提交预校验"去掉"对 jj 源且"限定后，未来若 reintroduce 非 jj 需改回 → 由 spec diff 记录留痕，可接受。
- **[herdr checkout_path 缺失频率]** 归一逻辑依赖 checkout_path 缺失时 pane cwd 可信 → herdr 若长期不提供 checkout_path 会频繁走回退，性能可忽略（无 spawn）。实现时以 spike 验证一次真实 workspace list 中 checkout_path 的覆盖率。

## Migration Plan

- 无数据/配置迁移。README wizard 段更新（删除非 jj fallback 句，说明候选仅限 jj 仓库、新 tab 恒建在 workspace 根），spec 归档时同步 main spec。
- 回滚：整体 revert 该 change 提交即可（纯代码 + 文档 + spec，无 schema 变更）。

## Open Questions

（无 —— 候选路径统一根已定案，O1 随 D1 闭环：`repo_root` 无需扩展祖先解析，`workspace_destination` 分组不受子目录影响。）
