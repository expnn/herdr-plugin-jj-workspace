# workspace-setup-script Specification

## Purpose
TBD - created by archiving change right-pane-script. Update Purpose after archive.
## Requirements
### Requirement: setup 脚本随插件分发
插件 SHALL 在仓库 `scripts/setup-workspace.sh` 提供静态 setup 脚本（`#!/bin/sh`，git 可执行位 100755），随 `plugin install` 与 `plugin link` 分发。脚本 MUST NOT 依赖生成逻辑或临时文件：其内容在插件版本内恒定。

#### Scenario: 脚本随安装可用
- **WHEN** 用户通过 `plugin install` 或 `plugin link` 安装插件
- **THEN** `<plugin_root>/scripts/setup-workspace.sh` 存在且可执行

### Requirement: 脚本自定位插件可执行文件
脚本 SHALL 经 `$0` 推导插件根目录并在 `<plugin_root>/target/release/jj-workspace` 定位插件可执行文件，用于后台启动 finish-tab 监视器。插件 exe MUST NOT 出现在脚本参数中。

#### Scenario: finish-tab 经自定位 exe 启动
- **WHEN** wizard 以绝对路径调用脚本并传入 workspace/tab/pane ID
- **THEN** 脚本以 `<plugin_root>/target/release/jj-workspace` 为 exe 后台启动 finish-tab（重定向与后台语义与改造前一致）

### Requirement: 脚本签名与调用等价性
脚本签名 SHALL 为 `setup-workspace.sh <jj路径> <base-rev> <书签名> <workspace-id> <tab-id> <pane-id> [前导参数...]`。脚本 MUST 以 `"$JJ_EXE" "$@" <子命令>` 的形态执行每次 jj 调用：尾部可变参（argv 形态 `jj.command` 的前导参数）展开为零个或多个词插入子命令之前。脚本调用形态 MUST 与插件进程内 `Command::new(exe).args(extras).args(subcmd)` 等价；string 形态（无前导参数）时 MUST 退化为裸 `"$JJ_EXE" <子命令>`。唯一例外为主仓库上下文解析调用：该调用 MUST 在 `"$JJ_EXE" "$@"` 之后、子命令之前追加全局选项 `-R <主仓库根> --ignore-working-copy`，其余调用 MUST NOT 追加额外全局选项。

#### Scenario: string 形态等价
- **WHEN** `jj.command` 为 string 形态，脚本以 `<jj路径> <base-rev> <书签名> <ws> <t> <p>` 被调用
- **THEN** 脚本对假 jj 的调用日志为 `sparse set --clear --add .`、`bookmark create <书签名> -r @`、`git fetch`、`-R <主仓库根> --ignore-working-copy log -r <base-rev> --no-graph -T 'commit_id ++ "\n"'`、`rebase -s @ -d <解析出的 union>`，顺序一致

#### Scenario: argv 形态前导参数传播
- **WHEN** `jj.command` 为 `["<路径>", "--at-op", "@-"]`，脚本尾部收到 `--at-op @-`
- **THEN** 每次 jj 调用均为 `<jj路径> --at-op @- [额外全局选项] <子命令>…`（前导参数出现在子命令之前；解析调用额外携带 `-R`/`--ignore-working-copy`）

### Requirement: 失败语义逐字保留
脚本 MUST 保留既有失败语义并新增解析降级语义：sparse 物化失败 MUST 中断后续全部步骤（脚本非零退出）；bookmark 创建失败 MUST 仅向 stderr 输出警告并继续 fetch/解析/rebase；`fetch` 或 `rebase` 失败 MUST 使脚本非零退出；主仓库根推导失败、解析调用非零退出或解析结果为空 MUST 仅向 stderr 输出警告并跳过 rebase（脚本继续执行并零退出）。

#### Scenario: 物化失败中断
- **WHEN** 假 jj 在 `sparse` 子命令上失败
- **THEN** 脚本立即非零退出，日志仅含 sparse 一次调用

#### Scenario: bookmark 失败仅警告
- **WHEN** 假 jj 在 `bookmark` 子命令上失败
- **THEN** 脚本仍成功执行 `git fetch`、解析与 `rebase` 并零退出，stderr 含书签警告文本

#### Scenario: 解析失败或空集降级
- **WHEN** 假 jj 的解析调用（`-R … log`）非零退出，或退出为零但输出为空
- **THEN** 脚本向 stderr 输出警告、不执行 `rebase`、以零退出结束

#### Scenario: rebase 失败仍非零退出
- **WHEN** 假 jj 在 `rebase` 子命令上失败
- **THEN** 脚本非零退出

### Requirement: pane 命令为单行脚本调用
wizard 生成的右侧 pane 命令 SHALL 为一行脚本调用（脚本绝对路径 + 逐参数 `shell_quote`），MUST NOT 包含拼接的 jj 命令序列、`sh_c_escape` 转义内容或对 pane shell PATH 的依赖。脚本路径 MUST 为绝对路径（基于 `HERDR_PLUGIN_ROOT`）。命令 MAY 携带来自 `[init]` 解析结果的可选前缀（`<init.right|init.default> && `，见 plugin-config 的 `[init]` 要求）：前缀为用户文本、原文注入、由 pane 的 shell 解析；无论是否携带前缀，脚本调用本身 MUST 仍是单次全参数引用调用，其参数形态 MUST NOT 因前缀存在而改变。

#### Scenario: 一行调用
- **WHEN** wizard 创建 jj 工作区且 `[init]` 未提供右侧非空命令
- **THEN** 右侧 pane 收到 `'<脚本绝对路径>' '<jj路径>' '<base-rev>' '<书签名>' '<ws>' '<t>' '<p>' [<'前导参数'>…]` 形态的单行命令，其中不含任何 `&&` 链或子 shell 组

#### Scenario: 携带 init 前缀
- **WHEN** wizard 创建 jj 工作区且 `init.right`（或 `init.default`）解析结果非空
- **THEN** 右侧 pane 收到 `<init 前缀> && '<脚本绝对路径>' '<jj路径>' …` 形态的单行命令，其中脚本调用部分与未配置时逐字节相同

### Requirement: base revset 在主仓库上下文重解析
脚本 SHALL 在 `jj git fetch` 之后、`rebase` 之前推导主仓库根并以主仓库上下文重解析 base revset：主仓库根 SHALL 由本工作区 `.jj/repo` 指针推导（指针相对 `.jj/` 解析、绝对路径原样使用，结果规范化；脚本签名不变、无新参数）；解析 SHALL 以 `jj -R <主仓库根> --ignore-working-copy log -r <base-rev> --no-graph -T 'commit_id ++ "\n"'`（每 commit 一行）进行，脚本 SHALL 将多行结果连接为 ` | ` 分隔的单一 union revset；解析结果非空时 `rebase -s @ -d` 的目标 MUST 为该 union（单参数），解析为空或失败时跳过（见失败语义）。`jj git fetch` 的位置与形态（副工作区、全量拉取）MUST 不变。

#### Scenario: 远程 bookmark 前进后落到最新 tip
- **WHEN** base 为 `main@origin`，创建后外部推进了远端 main，脚本的 `jj git fetch` 更新了 `main@origin`
- **THEN** 解析调用在主仓库上下文解析出更新后的 commit id，`rebase -s @ -d <新 id>` 把工作副本落到新 tip

#### Scenario: @ 相对 revset 与创建时上下文一致
- **WHEN** 用户把 base 设为 `@`（或 `@--` 等 `@` 相对表达式）并创建
- **THEN** 脚本解析出与 `workspace add -r` 相同的主仓库上下文结果；`@` 场景 rebase 为 no-op（不再出现 `Cannot rebase … onto itself`），`@--` 场景不再改基到错误 commit

#### Scenario: 多 commit 解析以 union 传入
- **WHEN** base revset 在主仓库上下文解析出多个 commit（如 `main@origin | dev@origin`）
- **THEN** 脚本执行 `rebase -s @ -d "<id1> | <id2>"`（单一 revset 参数，保留 merge-parents 语义）

