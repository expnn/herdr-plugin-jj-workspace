# Proposal: right-pane-script

## Why

创建 jj 工作区时，右侧 pane 收到的是一条经 `sh_c_escape` 深度转义的拼接长命令（nohup finish-tab + 4 段 jj 操作），用户在终端里无法看清它做了什么；为兼容 fish 等交互 shell 而引入的引号转义管道（`sh_c_escape` + 子 shell 包裹）复杂度高、测试面怪异（"命令串必须能在 fish 下 parse"）。将整个右侧 setup 收敛为一个随插件分发的、带注释的静态脚本，pane 只需调用一行脚本命令，可读性与可测试性同时提升。

## What Changes

- 新增静态脚本 `scripts/setup-workspace.sh`（`#!/bin/sh`，随插件仓库分发、git 保留可执行位）：后台启动 finish-tab 监视器，依次执行 sparse 物化、bookmark 创建（失败仅警告）、`jj git fetch`、`jj rebase -s @ -d <base-rev>`。
- 脚本签名：`setup-workspace.sh <jj路径> <base-rev> <书签名> <workspace-id> <tab-id> <pane-id> [前导参数...]`——尾部可变参承载 argv 形态 `jj.command` 的前导参数，脚本内经 `"$@"` 插入每次 jj 调用；插件 exe 不作为参数，由脚本经 `$0` 自定位 `<plugin_root>/target/release/jj-workspace`（相对布局由 manifest `[[build]]` 保证）。
- wizard 的右侧 pane 命令从拼接长串缩为一行脚本调用（各参数逐个 `shell_quote`）。
- 删除 `sh_c_escape` 转义管道与 `jj_shell_words` 拼接逻辑；`jj.command` 的解析唯一性语义不变（脚本仍接收解析后的绝对路径）。
- 测试调整：fish/sh parse 测试替换为对脚本自身的 `sh -n` 语法校验；假 `jj` 日志断言不变（行为回归守卫），并新增"脚本调用 ≡ 进程内调用形态"等价断言（含前导参数用例）。
- **行为不变**：`base-rev` 参数暂传 `'trunk()'`（per-repo-base-rev change 再参数化）；失败语义逐字保留（物化失败中断、bookmark 失败仅警告、fetch/rebase 照常）。

## Capabilities

### New Capabilities

- `workspace-setup-script`: 右侧 pane setup 脚本的定位、签名、调用等价性（脚本调用 ≡ 进程内调用）与失败语义。

### Modified Capabilities

（无 —— `plugin-config` 的解析唯一性需求在脚本形态下继续成立，无需修改。）

## Impact

- 新增 `scripts/setup-workspace.sh`（仓库内资产，随 `plugin install`/`plugin link` 分发）。
- `src/main.rs`：`jj_setup_command`/`right_pane_setup_command`/`jj_shell_words` 重写为脚本调用构造；`sh_c_escape` 删除；相关测试更新。
- `Cargo.toml`/配置 schema 无变化；README 的 Quickstart 段落同步（右侧脚本说明）。
