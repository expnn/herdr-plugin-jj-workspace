# Tasks: right-pane-script

## 1. 脚本

- [x] 1.1 新增 `scripts/setup-workspace.sh`：`#!/bin/sh` + 头部注释（用法与各步骤说明）；`$0` 自定位 `PLUGIN_ROOT` 与插件 exe；`shift 6` 后以 `"$JJ_EXE" "$@" <子命令>` 执行 sparse 物化、bookmark（`||` 警告）、fetch、rebase；`set -e` 于 nohup 行之后开启
- [x] 1.2 `chmod 100755` 并提交可执行位

## 2. main.rs 接入

- [x] 2.1 `open_tab_layout`：右侧 pane 命令改为单行脚本调用（`HERDR_PLUGIN_ROOT/scripts/setup-workspace.sh` 绝对路径 + `ResolvedJj.executable`/`extra_args`/`base-rev('trunk()')`/书签名/三个 ID 逐个 `shell_quote`；前导参数作为尾部可变参）
- [x] 2.2 删除 `jj_setup_command`、`right_pane_setup_command`、`jj_shell_words`、`sh_c_escape`
- [x] 2.3 `grep -n 'sh_c_escape\|jj_shell_words\|JJ_MATERIALIZE\|JJ_UPDATE' src/main.rs` 零命中（转义管道清除判据）

## 3. 测试

- [x] 3.1 删除 fish/posix parse 测试；新增对 `scripts/setup-workspace.sh` 的 `sh -n` 语法校验（路径相对 `CARGO_MANIFEST_DIR`）
- [x] 3.2 假 `jj` 测试改为运行真实脚本（`sh <脚本> <假jj> trunk() <名> w t p`）：物化失败中断、bookmark 失败仅警告、顺序断言、单引号书签名保留——四条日志断言逐字守护
- [x] 3.3 新增等价性测试：argv 形态尾部前导参数经脚本传播（日志含 `--at-op @-` 前缀）；脚本自定位插件 exe（假插件 exe 落于临时 `<root>/target/release/` 布局验证 `$0` 推导）

## 4. 文档与验证

- [x] 4.1 README Quickstart：右侧 pane 说明改为"运行 `scripts/setup-workspace.sh`（可 `cat` 复盘）"；排障节补"可执行位丢失"一行
- [x] 4.2 `cargo test` 全绿；`sh -n scripts/setup-workspace.sh` 通过
- [x] 4.3 手工验收（需真实 herdr 运行时）：创建工作区全流程行为与改造前一致；右侧终端可见脚本逐命令输出；故意制造 bookmark 冲突验证仅警告
