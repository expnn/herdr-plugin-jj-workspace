# Design: pane-init-hooks

## Context

- 现状：wizard 同步完成 `jj workspace add` + 引导文件 sparse 物化后，`open_tab_layout` 用 `herdr tab create` / `pane split` 建出左右两个 pane（cwd = 新工作副本），随后各发一条 `herdr pane run`：左 pane 注入 `agent.command`，右 pane 注入对 `scripts/setup-workspace.sh` 的单次全参数引用调用。`herdr pane run` 的语义是"向 pane 的 pty 粘贴一行文本 + 回车"（对端启用 bracketed paste 时整段作为粘贴），不等待、不解析、不创建 shell；shell 早由 tab/split 拉起。
- 现有扩展点只有 `agent.command`（把 init 文本塞进去即混用语义）与 `agent.bootstrap_paths`（只能物化文件，不能执行）；右 pane 无任何扩展点。
- 硬约束：pane 的 shell 是用户 `$SHELL`（herdr `terminal.default_shell` 为空时的回退链），可能是 fish/bash/zsh；右 pane 脚本是 `#!/bin/sh` 且其调用要求"单次全参数引用调用"（workspace-setup-script 既有 requirement）。
- 时机事实：两个 pane 的注入命令几乎同时发出、并发执行；右 pane 脚本的 `jj sparse set --clear --add .` 完成前，新工作副本只有引导文件。

## Goals / Non-Goals

**Goals:**

- 为两个 pane 各自提供一条"主命令执行前"的可选 shell 初始化命令，配置在 `config.toml` 的 `[init]` 节。
- 保持未配置时的既有行为逐字节不变（零行为变化的缺席态）。
- 给两个 pane 共用的初始化意图一个"写一次、两侧共用"的表达（`init.default`）。
- 失败可见且不静默：初始化失败阻断该 pane 的主命令，用户可在自己的 init 文本里显式软化（`|| true`）。

**Non-Goals:**

- checkout 完成后的初始化（装依赖/装 git hooks 等）：本 change 只做 pre 钩子；见 Open Questions。
- 不引入第二条配置通道（env/.env）；不用 `herdr tab create/split --env`（只能传静态环境变量，无法执行命令，且不满足"命令前初始化"的表达力）。
- 不做"失败回退到 `init.default`"（default 是缺席默认，不是失败兜底）。
- 不做 init 命令的语法校验/沙箱：与 `agent.command` 同为用户自担的 shell 文本。
- 不改 `setup-workspace.sh` 的签名与脚本内步骤。

## Decisions

### D1: 注入机制 = 单行 `<init> && <原命令>`，一次 `pane run`

每个 pane 仍只发一次 `herdr pane run`；解析结果非空时注入文本为 `<init> && <原命令>`。理由：

- **确定性**：`&&` 由 shell 串行执行，无"第二条文本何时被读入"的时序问题。
- **状态留存**：init 在 pane 的交互式 shell 内执行，`source`/`export`/`cd` 对随后启动的 agent（左侧）与该 pane 之后的人工使用（右侧终端）都有效。
- **缺席零变化**：不配置时注入文本就是原命令本身，可测"逐字节不变"。

被否决的备选：

- **两次 `pane run`（先 init 后主命令）**：`pane run` 不等待；第二条文本可能在 init 仍运行时到达 pty，被 init 进程/子进程从 stdin 读走，或落入子 shell（如 `nix develop`）上下文执行——不可靠。
- **把 init 作为参数/env 传进 setup 脚本、脚本内 `eval`**：初始化在 `/bin/sh` 子进程里执行，shell 状态不留在右 pane 的交互式 shell；且要扩签名或引入 env 传递通道，重新引入"用户文本二次求值/转义"的复杂度（旧右 pane 方案正因 `sh_c_escape` 复杂度被收敛掉）。
- **`tab create --env` / `pane split --env`**：只能传静态环境变量，无法表达命令/条件/激活脚本；也违背"config.toml 唯一配置通道"的项目约束方向。

### D2: 每 pane 独立键 + `default`（覆盖语义）

`[init]` 三键：`default`、`left`、`right`；每 pane 独立解析 `left → default → 无`（右同理）。理由：

- 两个 pane 的职责与失败域不同（左 = agent 运行环境；右 = jj checkout/网络环境），分开表达后一个失败不阻断另一个。
- pane 内没有可靠的身份信号可让共享脚本自行分支（herdr 只给 pane shell 注入 `HERDR_ENV=1`，无 pane/tab id），共享钩子无法分侧。
- 若只设单一共享键，任何非幂等副作用（如装依赖）会在两个 pane 并发执行两遍——危险默认值。`default` 是被显式选择的"两侧同用"，不是隐式双跑。

被否决的备选：`[init] command` + `panes = ["left","right"]` 数组枚举——schema 面更大（新增枚举/数组形态），且同样无法让一个脚本按 pane 分叉。

### D3: 值语义 = shell 命令行，原文注入

值是单个字符串、不做转义/二次引用，直接由 pane 的 shell 解析（与 `agent.command` 同一信任与注入模型）。理由：一条语义覆盖"纯命令"（`export ...`）、"激活环境"（`. /path/activate`、`source /path`）、"跑脚本"（`sh /path`）三种用法；若改为"脚本路径 + 插件代为执行"，插件必须替用户决定用哪个 shell、要不要 source，能力更小且多一层失败模式。代价：需要改变 shell 状态的 init 必须匹配该 pane 的 `$SHELL` 语法；跨 shell 通用用 `sh /path`（无状态留存）——写入 README。

### D4: 失败语义 = `&&` 阻断，不失败回退

初始化非零退出 → 该 pane 主命令不执行；用户要"尽力而为"就在自己的 init 文本里写 `|| true`。`&&` 严格强于 `;`（可反向表达，反之不行）。左 pane 失败时 agent 不启动，由既有 agent-readiness 超时通道提示（finish-tab 通知"未检测到 agent"）；右 pane 失败时错误留在右 pane、checkout 不跑（与该脚本今天 sparse 失败中断同类）。显式空串仍按全插件统一规则 fail-fast，"空"只能由键缺席表达；`default` 已设而想关闭单侧时，可写 `right = "true"` 作为显式 no-op（不为这两个键特批空串语义，避免统一规则出现特例）。

### D5: pre-only 边界

两个钩子都在各自 pane 主命令之前、全量 checkout 之前执行；spec 不承诺钩子执行时项目文件存在。post-checkout 插槽（`<脚本> && <init.post>`）本次不设计：其失败语义（不应影响已完成的 checkout？）与键名/解析链需要单独定案，避免在未明确的语义上做兼容承诺。

### D6: 规格落点与 delta 范围

- `plugin-config`：以 **ADDED** 新增"`[init]` 节为两个 pane 提供初始化命令前缀" requirement，不改写既有 "分节 schema 与内置默认值"（沿用 agent-readiness 先例：键组由专用 requirement 承载，既有 schema requirement 的枚举不逐个补列）。
- `workspace-setup-script`：**MODIFIED** "pane 命令为单行脚本调用"，加入 init 前缀 carve-out；顺带把该 requirement 场景里与实现不符的 `nohup` 前缀文案修正为现实的形态（`nohup` 在脚本内部，不在 pane 命令里）。

## Risks / Trade-offs

- [用户把依赖项目文件的初始化放进 pre 钩子 → 必然失败并阻断主命令] → spec 明确 pre-only 边界，README 用一句"钩子执行时工作副本只有引导文件"说明；失败输出留在 pane 可见。
- [shell 可移植性：用户在 fish pane 写 bash 专有语法] → README 说明"需要留状态的 init 必须匹配 pane 的 `$SHELL`；跨 shell 用 `sh /path`"。
- [右 pane 命令不再是纯脚本调用（放宽既有 MUST NOT 措辞）] → carve-out 限定"前缀为用户文本、原文注入；脚本调用本身仍是单次全参数引用调用"，并在测试中断言携带前缀时脚本调用部分逐字节不变。
- [`default` + 显式关闭缺口（无法用空表达"这侧不要"）] → 文档给出 `right = "true"` 变通；不特批空串。
- [长耗时 init 延迟右 pane 脚本（进而延迟 finish-tab 监视器启动）] → 分析：超时时钟自 watcher 启动起算，不会误报"未检测到 agent"；tab 创建时已 focus，焦点拉取只是兜底。README 建议 init 保持短小。
- [init 文本以 `&` 结尾等边界 → `<init> && main` 会出现语法错误] → 属用户文本边界，README 建议复杂初始化写成脚本文件（`sh /path`）。

## Migration Plan

纯增量：不配置 `[init]` 时行为逐字节不变，无需迁移。回滚 = 移除 `[init]` 配置节（或回滚版本），无数据/状态残留。

## Open Questions

- checkout 完成后的初始化（post 钩子：装依赖、装 git hooks、`direnv allow`）是否立项为独立 change？不影响本 change 的 specs、实现与任务拆分。
