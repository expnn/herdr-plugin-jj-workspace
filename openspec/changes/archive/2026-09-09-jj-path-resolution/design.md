# Design: jj-path-resolution

## Context

change #1（config-foundation）建立了 `config.toml` 解析基座与本 change 需要的分节 schema（`[jj]` 节）。本 change 解决：herdr server 进程 PATH 精简（`herdr --remote` 自举场景）导致插件进程内 `jj` 调用失败的故障模式。

插件内 `jj` 的执行上下文有两类（`src/main.rs` 已核对）：

```
┌──────────────────────────────┬─────────────────────────────────────┐
│ 同步调用（wizard 内           │ std::process::Command — 插件进程内   │
│ workspace add / sparse set）  │ 直接 exec，依赖 server PATH          │
├──────────────────────────────┼─────────────────────────────────────┤
│ 右侧 pane 的 jj 命令串        │ herdr pane run 敲进用户交互 shell    │
│ (jj_setup_command :388，      │ （:353）——目前为裸 "jj ..."，       │
│  right_pane_setup_command:412)│ 由 pane shell 的 PATH 解析           │
└──────────────────────────────┴─────────────────────────────────────┘
```

若两处各自解析，同一配置在两个上下文语义不一致。本 change 确立"一次解析、处处使用"。

## Goals / Non-Goals

**Goals:**

- `jj.command` 键落地（`String | Vec<String>`），默认 `"jj"`，含完整路径解析与校验规则。
- 失败语义与 config-foundation 的 fail-fast 通道完全一致（wizard TUI 报错 / action 非零退出 + toast），错误信息含修复指引。
- 右侧 pane 命令串烘焙解析后的绝对路径，消除双上下文解析歧义。
- 路径解析逻辑可单元测试（注入 PATH 与临时目录 fixture）。

**Non-Goals:**

- 不做 login-shell 自动探测（`$SHELL -lc 'command -v jj'`）——用户明确否决；解析事实源只有"插件进程环境 + 显式配置"。
- 不做跨进程缓存（每次插件调用解析一次，开销可忽略）。
- 不涉及 `herdr` 二进制解析（`HERDR_BIN_PATH` 由 herdr 注入，可靠）。
- 不涉及 `agent.command`（它是 shell 行注入模型，无路径解析需求）。

## Decisions

### D1: 解析规则矩阵

| 值形态 | 行为 |
|---|---|
| 裸名（无 `/`，如 `jj`） | 遍历 `$PATH` 各目录：条目存在 + 普通文件 + 可执行位（`PermissionsExt::mode() & 0o111`）→ 命中即返回绝对路径；目录不存在/空段静默跳过 |
| 绝对路径（`/` 开头） | 原样使用，仍执行"存在 + 普通文件 + 可执行位"校验 |
| 含 `/` 的相对路径 | **拒绝**，fail-fast：错误说明只接受裸名或绝对路径 |
| `~` 开头 | 先展开（与 `jj.workspace_root` 同一展开实现），再按上表处理 |
| argv 列表 | 仅 `argv[0]` 应用上表；其余元素原样保留 |

**理由**：相对路径相对于插件进程 cwd（插件安装目录），语义几乎必然不是用户所想，拒绝比猜测安全；argv 形态下只有 argv[0] 是可执行文件，逐元素检查徒增错误面。

### D2: 显式解析而非依赖 Command 的隐式 PATH 查找

**选择**：插件自己实现 PATH 查找，得到绝对路径后处处使用；不依赖 `std::process::Command` 的隐式解析。

**理由**：① 右侧 pane 命令串需要绝对路径文本，隐式解析给不出来；② 错误信息可以列出已搜索的目录与修复建议，而 `Command` 只给笼统的 `NotFound` io::Error；③ 校验（可执行位）前置，避免"解析成功但 exec 失败"的半途错误。

### D3: 解析唯一性——wizard 入口解析一次

**选择**：`cmd_wizard` 在加载配置后立即 `resolve_jj_command()`，结果（`PathBuf` 或 argv 头元素）进程内贯穿使用；`cmd_remove` 同理。`jj_setup_command` / `right_pane_setup_command` 接受解析结果参数，输出绝对路径版本（sh 转义规则不变，沿用 `sh_c_escape` 并补对应单测）。

**明确接受的后果**：server PATH 找不到 jj 时插件整体失败，即使右侧 pane 的用户 shell 能找到。修复手段统一为"配置 `jj.command` 绝对路径"。
**替代方案**：右侧 pane 保留裸 `jj` 由用户 shell 解析 —— 被否，双上下文解析使配置语义不可预测，且失败点后移（pane 里报错用户体验差）。

### D4: 失败信息结构

错误信息 MUST 包含：配置值原文、（裸名时）已搜索的 PATH 目录列表、修复建议（"在你的 shell 中运行 `which jj`，将结果写入 config.toml 的 `jj.command`"）。argv 形态错误信息额外注明"argv[0]"。

## Risks / Trade-offs

- [server PATH 缺 jj 但用户 shell 有 → 插件仍失败] → 刻意为之（D3）；错误信息直指修复路径，一次性配置成本。
- [PATH 中出现不可执行的同名文件（如文档目录）] → 可执行位校验跳过它继续查找，最终失败时错误信息列出全部已搜目录，可定位。
- [解析逻辑测试受全局 PATH 环境变量影响] → 解析函数接受 `env: impl Iterator<Item=(OsString,OsString)>` 或直接接受路径列表参数，测试注入临时目录列表，不碰进程 env。
- [change 依赖 config-foundation 的 schema] → 实施顺序固定：config-foundation 先行；tasks 中标注前置检查。

## Migration Plan

1. 依赖 config-foundation 落地（`load_config()` + `JjConfig` 存在）。
2. 本 change 单 commit 可 revert：去掉 `jj.command` 键（`deny_unknown_fields` 会开始拒绝它——rollback 时需同步提示用户删除该键；README 排障节注明）。
3. 行为默认值不变（`"jj"` + PATH），未配置用户无感。

## Open Questions

（无 —— 解析规则矩阵、argv[0] 规则、相对路径拒绝、无 login-shell 探测均已与用户定案。）
