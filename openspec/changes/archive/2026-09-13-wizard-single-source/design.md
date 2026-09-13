## Context

当前 wizard 的 source 来自 `load_workspace_choices_with`：全量拉取 `herdr workspace list` + `herdr pane list`，按 `active_tab_id` 只取 active tab 的 focused/首个 pane，经 `jj_root()` 上溯过滤，非 jj 丢弃。同一 herdr workspace 最多一条候选；非 active tab 的 jj 仓库不可见。`cmd_open` 经 `--env CURRENT_HERDR_WORKSPACE_ID` 把调用者 workspace id 转发给 wizard pane 做初始选中。

herdr 侧已核实（源码证据）：
- 插件 action 进程与 plugin pane 进程均被注入 `HERDR_PLUGIN_CONTEXT_JSON`（`runtime.rs:47`；`panes.rs:262`），其中 `focused_pane_cwd = pane.cwd`（`context.rs:368`，shell OSC 7 上报目录，回退 shell pid 的 process_cwd）。
- wizard pane 的 `placement = "overlay"`（本插件 manifest），overlay 打开路径用 `current_plugin_context("plugin-pane")`（`panes.rs:51`）——在 spawn 之前构造，指向调用者 pane，不受 `--focus` 影响；该 key 为受保护 env，调用方无法伪造（`panes.rs:346-359`）。
- 官方文档推荐口径（`plugins.mdx:272-275`）：常用 id 读单 env，完整形状解析 `HERDR_PLUGIN_CONTEXT_JSON`。
- 本机 herdr 0.8.2 的 `workspace list` 恒无 `checkout_path`，旧管线的真实热路径本就是 pane 回退——新设计直接取 context，去掉中间商。

约束：不新增配置通道（#131）；headless 错误走 `die()` toast（#149）；副 workspace 经 `repo_root()` 消解主根（`.jj/repo` 指针规则）；未发布不 bump version（#160 例）。

## Goals / Non-Goals

**Goals:**
- wizard 源收敛为调用者聚焦 pane 消解出的单一主仓库根，只读展示，无列表、无过滤、无空态。
- 上下文获取零 herdr CLI 回调：两个入口各自读自身注入的 context。
- 非 jj 时 `cmd_open` toast 报错并退出（不开 pane）；wizard 入口 fail-fast modal 兜底。
- 删光候选管线（`load_workspace_choices*`、`herdr_json_with`、`WorkspaceChoice`、假 herdr 测试装置、`--env` 转发）。

**Non-Goals:**
- 不改 base 解析链本身（`resolve_base_rev`）、`repo_root`/`jj_root` 算法、setup 脚本、remove/migration。
- 不恢复"非 jj 打开同文件夹" fallback（source-jj-only 已删除）。
- 不引入 `foreground_cwd` 通道（context 不提供；语义上也不想要）。
- 不做 version bump。

## Decisions

1. **两个入口各自读自身 `HERDR_PLUGIN_CONTEXT_JSON.focused_pane_cwd`，不经 `--env` 转发任何自定义变量**（备选：cmd_open 消解后转发 resolved root）。
   Rationale：文档推荐的 idiomatic 形状；wizard 自足（直开 pane entrypoint 也可用）；删掉整条自定义 env 通道（含既有的 `CURRENT_HERDR_WORKSPACE_ID`，`workspace_id` 同样从自身 context 取）。代价是消解逻辑跑两次（cmd_open 预检 + wizard 展示），但只是两次廉价 fs 上溯，且是同一对已测 helper。action-dispatch 与 pane-open 之间的 focus 竞态在同一按键流内可忽略，wizard 侧 fail-fast modal 覆盖发散。
2. **预检在 `cmd_open`，`die()` toast；wizard 只做 modal 兜底**（备选：全部报错收敛到 wizard modal）。
   Rationale：只有 headless action 侧有 toast 出口（#149）；用户明确要求"Toast 报错并退出"。modal 兜底覆盖绕过 action 的直开路径。
3. **源目录语义 = `pane.cwd`，不取 `foreground_cwd`**（备选：`herdr pane get $HERDR_PANE_ID` 取 `foreground_cwd || cwd` 延续旧口径）。
   Rationale：context 只提供 `pane.cwd`；它是 herdr 自身 label/follow-cwd 的权威口径，更稳定；旧口径的 foreground_cwd 优先曾导致 pyright dist 类仓库外深目录隐患（#156）。`jj_root` 上溯天然容忍子目录。
4. **展示、base 解析、`workspace add` cwd、dest 命名共用消解后的主仓库根这一个值**（备选：jj 执行仍用 pane 所在 workspace 根、仅展示主根）。
   Rationale：commit/bookmark 共享存储（#142）、repo 级配置同 hash 共享（#130），在主根执行与在副 workspace 执行等价；单值链路最短、最可测。
5. **Tab 循环收敛为 Name ↔ Base；Source/Checkout 只读，标题恒 subtext0 粗体**（用户指定顺序 New Workspace → Base → Source → Checkout）。
   Rationale：Source 不可编辑后无进入焦点的理由；Checkout 已有同款先例（spec 既有约束）。
6. **测试策略：删假 herdr fixture，新增纯函数单测**——`resolve_source(cwd)` 风格：jj 主根 / 副 workspace / 子目录 / 非 jj / `focused_pane_cwd` 缺失 / 空；wizard 渲染单测：section 顺序、Source 只读、Tab 循环不含 Source。

## Risks / Trade-offs

- [action-time 与 pane-open-time context 理论发散] → 同一按键流毫秒窗口；wizard fail-fast modal 覆盖坏源。
- [global context 下无 active workspace（ctx 空）] → `cmd_open` 直接 `die()` toast，本来就没有可建 tab 的 workspace，行为正确。
- [OSC 7 滞后 / shell 集成缺失] → herdr 侧有 process_cwd 回退；`jj_root` 上溯容忍子目录；极端 stale 只影响"当前子目录"精度，不影响仓库判定。
- [BREAKING：其他 tab 的仓库不可选] → 接受；用户先切到对应 pane 再触发 action，README 写明。
- [直开 wizard pane 拿不到 toast] → modal 文案与 toast 同源（一行摘要 + error.log 指针），可复制性反而更好。
