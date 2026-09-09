# Design: agent-decoupling

决策 D1-D5。调研依据：herdr 源码（detect 封闭 enum + per-agent manifest、agent_status 状态机 Working→Idle→Done、Blocked=等待人工输入、`agent list` 恒为 JSON envelope）——结论存于项目记忆 #134/#137/#138。

## D1 分层就绪状态机（替代全部文案爬取）

```
loop（poll_interval_ms 间隔，≤ startup_timeout_secs 总预算）:
    agents = herdr agent list（JSON envelope）
    entry  = agents 中 pane_id == 左 pane 的条目       ← 不过滤 agent 标签
    match entry:
        None      → continue                            ← agent 尚未被检测到
        Some(entry):
            label  = entry.agent（运行时检测标签）
            status = entry.agent_status
            if status == "blocked" 且 auto_trust 允许（见 D2）→ 回车，continue
            if status == "blocked" 且持续 ≥ blocked_grace_secs(内部常量 1s)
                → fail("{label} needs attention")
            else（Working/Idle/Done/任意非 blocked）
                → focus 循环（workspace/tab/agent focus，不变）→ exit 0
超时 → fail("{label} needs attention")
```

关键语义：**条目出现 = agent 已被 herdr 进程树检测到 = 可聚焦**。不再等任何"就绪文案"稳定——过早聚焦对 agent 启动无实际影响，等待它是纯开销。blocked 需要稳定 1s 才判定（防瞬时状态闪变误报）。

被否方案：等 Working/Idle 稳定才聚焦（多等一轮状态、无实际收益）；就绪文案爬取（本次要删除的东西）。

## D2 auto-trust 门控：codex-only + 启动时间窗 + opt-in

回车的全部条件（缺一不可）：

1. `agent.auto_trust == true`（默认 **false**——不对任何 agent 做未授权应答是默认行为）
2. 运行时检测标签 `== "codex"`（标签来自 agent list，非配置声明）
3. `status == "blocked"`
4. 处于启动窗口内（elapsed < `trust_window_secs`，从 finish-tab 启动算起）

依据：finish-tab 只活 `startup_timeout_secs`、只存在于启动阶段；codex 的信任弹窗必然是启动后第一个 blocked（权限提示仅在 agent 执行命令后出现，那时 finish-tab 已退出）。窗口把"无差别回车按错默认选项"的残余风险收窄到"用户改过 codex 配置使启动即弹非信任 prompt"的边缘场景。

成功确认以**状态转移**为准：回车后回到非 blocked → 继续主循环；持续 blocked → 重试（上限 `trust_max_attempts = 5` 内部常量），超限后按通用 blocked 语义 fail。

被否方案：全程无差别回车（B1，权限提示默认选项可能为 No，语义错误）；herdr 事件驱动（D4）。

## D3 轮询而非事件驱动

herdr 的 manifest 事件（`pane.agent_status_changed`）模型是"每次事件 spawn 一个短命 command"；finish-tab 是插件自启的长驻进程，无法订阅事件。事件化意味着把状态机拆散到多个被 spawn 的短命进程（状态跨进程传递 + 竞态），复杂度陡增而收益（省 100-200ms 间隔的 `agent list` 子进程）可忽略（20s 预算内 ~100 次）。保守轮询保留单进程状态机。

被否方案：`pane.agent_status_changed` 事件化——除非将来放弃 finish-tab 形态，否则不复议。

## D4 配置 schema（[agent] 节扩展）

| 键 | 类型 | 默认 | fail-fast 校验 |
|---|---|---|---|
| `auto_trust` | bool | `false` | 非 bool → 类型错误带键名 |
| `trust_window_secs` | 整数秒 | `10` | 必须 ≥ 1 |
| `startup_timeout_secs` | 整数秒 | `20` | 必须 ≥ 1 |
| `poll_interval_ms` | 整数毫秒 | `200` | 必须 ≥ 10 |

时间类键用 `_secs`/`_ms` 后缀显式标单位、纯整数——不引入时长字符串解析。下限校验防退化配置（如 0 = 忙轮询/立即超时）。

内部常量（刻意不进 config）：`blocked_grace_secs = 1`（防瞬时 blocked 误报）、`trust_max_attempts = 5`（防病态回车循环）。配置面收益低、正常用户无感知差异。

`auto_trust` 沿用 config-foundation 预留的占位键名与 bool 形态——从占位转正，非 breaking（此前写入会被硬拒绝）。**行为 breaking**：默认 false 意味着存量 codex 用户不再被自动回车（proposal 已标注）。

## D5 finish-tab 接入配置加载

`cmd_finish_tab` 此前被 config-foundation 豁免（不读配置）；现在读 4 个新键，需补 `load_config()` fail-fast（解析/未知键/空值错误 → stderr + 非零退出，与 open/remove 一致）。

连带删除：`wait_for_codex_and_accept_trust` 的两段文案匹配、`read_pane_text` 函数、`herdr agent list` 过滤中的 `agent == "codex"`（改为取任意条目的标签）、toast 硬编码文案（改为 `format!("{label} needs attention")`）。

## 测试策略

- `herdr agent list` 的 JSON envelope 用注入的假 `herdr` 可执行文件模拟（输出 fixture JSON，可控制条目出现/缺失/状态转移序列）——不碰真实 herdr、不碰进程 env
- 状态机测试：条目延迟出现、blocked 稳定判定、auto-trust 三重门控（默认关闭/非 codex/窗口外）、回车后状态转移确认、超时
- 配置层：4 个新键的解析/类型错误/下限拒绝
- 手工验收：真实 herdr 下 codex（auto_trust=false 手动回车 / true 自动回车）与非 codex agent 各跑一遍创建流程
