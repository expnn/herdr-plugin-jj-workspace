# Delta Spec: workspace-setup-script

## ADDED Requirements

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
脚本签名 SHALL 为 `setup-workspace.sh <jj路径> <base-rev> <书签名> <workspace-id> <tab-id> <pane-id> [前导参数...]`。脚本 MUST 以 `"$JJ_EXE" "$@" <子命令>` 的形态执行每次 jj 调用：尾部可变参（argv 形态 `jj.command` 的前导参数）展开为零个或多个词插入子命令之前。脚本调用形态 MUST 与插件进程内 `Command::new(exe).args(extras).args(subcmd)` 等价；string 形态（无前导参数）时 MUST 退化为裸 `"$JJ_EXE" <子命令>`。

#### Scenario: string 形态等价
- **WHEN** `jj.command` 为 string 形态，脚本以 `<jj路径> <base-rev> <书签名> <ws> <t> <p>` 被调用
- **THEN** 脚本对假 jj 的调用日志为 `sparse set --clear --add .`、`bookmark create <书签名> -r @`、`git fetch`、`rebase -s @ -d <base-rev>`，顺序一致

#### Scenario: argv 形态前导参数传播
- **WHEN** `jj.command` 为 `["<路径>", "--at-op", "@-"]`，脚本尾部收到 `--at-op @-`
- **THEN** 每次 jj 调用均为 `<jj路径> --at-op @- <子命令>…`（前导参数出现在子命令之前）

### Requirement: 失败语义逐字保留
脚本 MUST 保留改造前的失败语义：sparse 物化失败 MUST 中断后续全部步骤（脚本非零退出）；bookmark 创建失败 MUST 仅向 stderr 输出警告并继续 fetch/rebase；fetch 或 rebase 失败 MUST 使脚本非零退出。

#### Scenario: 物化失败中断
- **WHEN** 假 jj 在 `sparse` 子命令上失败
- **THEN** 脚本立即非零退出，日志仅含 sparse 一次调用

#### Scenario: bookmark 失败仅警告
- **WHEN** 假 jj 在 `bookmark` 子命令上失败
- **THEN** 脚本仍成功执行 `git fetch` 与 `rebase` 并零退出，stderr 含书签警告文本

### Requirement: pane 命令为单行脚本调用
wizard 生成的右侧 pane 命令 SHALL 为一行脚本调用（脚本绝对路径 + 逐参数 `shell_quote`），MUST NOT 包含拼接的 jj 命令序列、`sh_c_escape` 转义内容或对 pane shell PATH 的依赖。脚本路径 MUST 为绝对路径（基于 `HERDR_PLUGIN_ROOT`）。

#### Scenario: 一行调用
- **WHEN** wizard 创建 jj 工作区
- **THEN** 右侧 pane 收到 `nohup <脚本绝对路径> '<jj路径>' '<base-rev>' '<书签名>' '<ws>' '<t>' '<p>' [<'前导参数'>…]` 形态的单行命令，其中不含任何 `&&` 链或子 shell 组
