# Tasks: per-repo-base-rev

## 1. 前置

- [x] 1.1 确认 right-pane-script 已实施：脚本 `$2` 参数位存在（当前传 `'trunk()'`），假 `jj` 测试运行真实脚本
- [x] 1.2 spike 复核（记忆 #130 已载）：`jj config get herdr.base-rev` 副工作区可读、键缺失 exit 1

## 2. 解析链

- [x] 2.1 实现 `resolve_base_rev(config: &Config, repo: &Path) -> String`：`jj config get herdr.base-rev`（`Command::new(解析后的jj.exe).current_dir(repo)`，非零退出 → 回退 `config.jj.base_rev` → `trunk()`）；单元测试用假 jj 脚本模拟 exit 0（输出值）/exit 1（缺失）两种形态
- [x] 2.2 `cmd_wizard`：wizard 打开时对初始 source 求值一次用于预填（结果仅作显示，不作提交依据）

## 3. wizard base 字段

- [x] 3.1 `WizardField::Base` 变体：Tab 循环三字段、光标渲染、`replace_on_type` 式首键替换（与 name 字段同策略）、预填值来自 2.2
- [x] 3.2 非 jj 源置灰：dim 样式 + 警告行追加"非 jj 源忽略此项"；提交时非 jj 源跳过 base
- [x] 3.3 dirty 标志：base 字段编辑即置位；提交时 dirty=false 按**最终选中** source 现算解析链，dirty=true 采用输入值
- [x] 3.4 revset 提交校验（仅 jj 源且 dirty=true）：`jj log -r <expr> --no-graph --limit 0 --no-pager`（cwd = source 仓库），非零退出将 stderr 写入错误行并停留
- [x] 3.5 单元测试：三字段 Tab 循环、dirty 分支求值逻辑（假 jj 注入）、置灰样式分支、校验失败错误文本传递

## 4. 接线与闭环

- [x] 4.1 脚本调用第二参数从 `'trunk()'` 换为提交时解析/校验后的 revset（`shell_quote` 传参）；`workspace add -r` 同值（argv 直传）
- [x] 4.2 更新 `setup_rebase_target_remains_trunk` 守卫测试：断言改为"rebase 目标 == 解析链结果"（默认配置下仍为 trunk()）
- [x] 4.3 假 `jj` 测试新增 revset 形态用例（含空格与单引号的 revset 经脚本传递的日志断言）

## 5. 文档与验证

- [x] 5.1 README：三种 base-rev 配置场景示例（`--repo` / `--user` / config.toml）+ base 字段说明（置灰、预校验、dirty 语义）
- [x] 5.2 `cargo test` 全绿；`sh -n scripts/setup-workspace.sh` 通过
- [x] 5.3 手工验收（需真实 herdr 运行时）：仓库 A/B 各设不同 base-rev 验证隔离；wizard 内输入非法 revset 被拦留；默认路径行为与改造前逐字节一致
