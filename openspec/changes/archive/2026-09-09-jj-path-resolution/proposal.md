# Proposal: jj-path-resolution

## Why

herdr server 的进程环境随启动方式漂移——通过 `herdr --remote <host>` 自举 server 时 PATH 是精简的，`jj` 往往不在其中。插件进程内所有 `jj` 调用（wizard 的 `jj workspace add`/`jj sparse set`、`remove` action）直接依赖该 PATH，导致创建工作区直接报错。需要给用户提供显式指定 jj 可执行路径的能力，并让整个插件（包括右侧 pane 的 shell 命令串）使用唯一一次解析的结果。

## What Changes

- 新增配置键 `jj.command`（`String | String[]`，change #1 的分节 schema 内）：`string` = 可执行文件（裸名或绝对路径，不含参数），`string[]` = 完整 argv（仅 argv[0] 参与路径解析）。
- 新增路径解析规则：裸名（不含 `/`）在插件进程 PATH 中逐目录查找（存在 + 普通文件 + 可执行位）；绝对路径原样使用（仍校验可执行）；含 `/` 的相对路径（如 `./bin/jj`）**拒绝**（fail-fast）；`~` 开头先展开再按上述规则处理。默认值 `"jj"`。
- argv 列表形式仅对 `argv[0]` 应用解析规则，其余元素原样拼接。
- 可执行检测失败时 fail-fast：wizard TUI 渲染错误、headless action 非零退出 + toast；错误信息包含配置值、已搜索的 PATH 目录列表与修复建议（"在你的 shell 中 `which jj`，将结果写入 `jj.command`"）。
- **解析唯一性**：wizard 入口解析一次、进程内缓存；右侧 pane setup 命令串中所有裸 `jj` 替换为解析出的绝对路径（`jj_setup_command` / `right_pane_setup_command`，`src/main.rs:388/412`）——右侧 shell 的 PATH 差异从此无关。
- 明确后果（写入 spec 与 README）：server PATH 找不到 jj 时插件整体失败，即使右侧 pane 的用户 shell 能找到——修复手段统一为配置 `jj.command` 绝对路径。
- 不做 login-shell 自动探测；`herdr` 二进制不走此机制（`HERDR_BIN_PATH` 由 herdr 注入，可靠）。

## Capabilities

### New Capabilities

（无 —— 全部变更归属既有能力 `plugin-config`，由 change #1 引入。）

### Modified Capabilities

- `plugin-config`: 新增 `jj.command` 键与路径解析语义；扩展 fail-fast 失败面（可执行检测失败）；新增"解析唯一性"约束（右侧 pane 命令烘焙绝对路径）。

## Impact

- `src/main.rs`：新增 `resolve_jj_command()`（解析 + 校验 + 错误构造）；`cmd_wizard` / `cmd_remove` 入口调用；`jj_setup_command` / `right_pane_setup_command` 的裸 `jj` 替换为绝对路径；现有 `sh_c_escape` 单测同步更新。
- `openspec/changes/config-foundation` 中的 schema（`JjConfig`）增加 `command` 字段——本 change 依赖 config-foundation 先落地（或同分支顺序实施）。
- README：Configuration 章节补 `jj.command` 说明与 PATH 失败的排障指引。
- 无对 herdr 侧的任何变更。
