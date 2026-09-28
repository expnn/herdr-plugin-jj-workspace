# Proposal: pane-init-hooks

## Why

插件创建 jj workspace 时，左右两个 pane 各自立刻执行一条注入命令（左：`agent.command`；右：随插件分发的 `scripts/setup-workspace.sh` 调用），用户没有任何"主命令执行前的初始化钩子"：目前唯一的变通是把 init 文本塞进 `agent.command`（把环境准备与 agent 启动混在同一个值里），而右 pane 完全没有扩展点——例如 `jj git fetch` 所需的代理/凭据环境无处注入。两个 pane 共享同一初始化意图时，也没有"写一次、两侧共用"的表达。

## What Changes

- 新增 `[init]` 配置节，三个可缺席的字符串键（值为 shell 命令行，原文注入 pane 的交互式 shell，与 `agent.command` 同一模型）：
  - `init.default`：两个 pane 的默认初始化命令；
  - `init.left` / `init.right`：分别覆盖左 / 右 pane。
- 每个 pane **独立**解析：`left → default → 无`（右：`right → default → 无`）。显式键**覆盖** `default`（覆盖，非叠加、非拼接）；全缺席 = 不做初始化。
- 解析结果非空时，该 pane 仍只发**一次** `herdr pane run`，注入文本为 `<init> && <原命令>`；init 失败由 `&&` 阻断该 pane 主命令（用户可自写 `|| true` 软化）。**不提供**失败回退到 `default` 的语义（`default` 是"缺席默认"，不是"失败兜底"）。
- **范围（pre-only）**：两个钩子都在各自 pane 主命令之前、且在全量 checkout 之前执行；此时新工作副本只有引导文件，钩子不能依赖项目文件。"checkout 完成后的初始化"（装依赖/装 git hooks 等）不在本 change 范围。
- 显式空串仍按全插件统一规则 fail-fast（`init.*` 不允许空值）："空"只能由键缺席表达；不新增 env/.env 通道；不提供数组形式。
- 不改变未配置 `[init]` 时的任何行为（两个 pane 的注入文本逐字节不变）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `plugin-config`：新增一条 requirement——`[init]` 节的键、每 pane 解析链、pane 命令组合规则（单次 `pane run`、`&&` 失败语义、pre-only 边界、空串拒绝）。用 ADDED 新增，不改写既有 requirement（沿用 agent-readiness 先例：键组由专用 requirement 承载）。
- `workspace-setup-script`：MODIFIED "pane 命令为单行脚本调用"——加入可选 init 前缀的 carve-out（前缀为用户文本原文注入；脚本调用本身仍是单次全参数引用调用），并顺带修正该 requirement 场景中与实现不符的 `nohup` 前缀旧文案（现实现无 `nohup` 前缀，`nohup` 在脚本内部）。

## Impact

- `src/main.rs`：`Config` 新增 `[init]` 解析与校验；新增每 pane 解析与命令组合纯函数；`open_tab_layout` 用组合结果做左、右两条 `pane run`（缺席时逐字节不变）。
- `README.md`：Configuration 节新增 `[init]` 示例、键说明、覆盖语义、`&&` 失败语义（可 `|| true`）、pre-only 边界与 shell 可移植性（`sh /path` vs `. /path`）。
- `openspec/specs/plugin-config/spec.md`、`openspec/specs/workspace-setup-script/spec.md`（经 delta 归档）。
- 测试：配置解析/校验（缺席、覆盖、空串拒绝）、命令组合（有/无 init、两侧独立、default 回退、缺席逐字节不变）、fake herdr 集成断言两条 `pane run` 的完整注入文本。
