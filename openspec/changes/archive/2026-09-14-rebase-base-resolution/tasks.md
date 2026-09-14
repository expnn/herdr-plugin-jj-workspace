## 1. setup 脚本（主根推导 + 主上下文解析）

- [x] 1.1 `scripts/setup-workspace.sh` 新增主仓库根推导：读取当前目录 `.jj/repo` 指针（相对 `.jj/` 解析、绝对路径原样使用、`pwd -P` 规范化；不可读时置空），同步更新头部注释中对第 5 步的说明——验证：`sh -n scripts/setup-workspace.sh` 通过
- [x] 1.2 第 5 步改为主上下文解析：`"$JJ_EXE" "$@" -R "$MAIN_ROOT" --ignore-working-copy log -r "$BASE_REV" --no-graph -T 'commit_id ++ "\n"'`（每 commit 一行，脚本侧连接为 union revset；原 `separate(" | ", commit_id)` 模板被实测证伪——多 commit 输出无分隔符）；解析非零退出或输出为空时向 stderr 输出警告并跳过 rebase——验证：降级用例 2.3 通过
- [x] 1.3 rebase 改为 `"$JJ_EXE" "$@" rebase -s @ -d "$DEST_REV"`（多 id 由 union 串联）；fetch 与 sparse/bookmark 步骤保持原位不变——验证：顺序断言用例 2.2 通过

## 2. setup 脚本测试夹具与用例

- [x] 2.1 扩展 `run_setup_script` 夹具（`src/main.rs:2700`）：脚本 cwd 指向含 `.jj/repo` 指针的 dest 目录（指针指向临时 main 目录）；假 jj 增加可配置行为（解析输出、按子命令失败、`rebase` 失败）——验证：既有 `setup_*` 用例在新夹具下运行
- [x] 2.2 更新既有断言：`setup_runs_materialize_bookmark_fetch_rebase_in_order` 的调用日志改为 5 步（sparse / bookmark / fetch / `-R <主根> --ignore-working-copy log …` / `rebase -s @ -d <ids>`）；`setup_propagates_leading_args_before_each_subcommand` 覆盖解析调用的 `--at-op @-` 前缀；`setup_passes_base_rev_to_rebase` 与 `setup_passes_revsets_with_spaces_and_quotes` 改为断言解析调用收到完整 revset 单词——验证：`cargo test` 中相关用例全绿
- [x] 2.3 新增解析降级用例：假 jj 解析调用非零退出 → stderr 警告、无 `rebase` 调用、脚本零退出；解析退出为零但无输出 → 同上——验证：两个用例断言日志与 stderr
- [x] 2.4 新增 union 用例：假 jj 解析输出两个 commit id → 断言单参数 `rebase -s @ -d "<id1> | <id2>"`——验证：用例通过
- [x] 2.5 新增 rebase 失败用例：假 jj `rebase` 子命令失败 → 脚本非零退出——验证：用例通过
- [x] 2.6 新增指针用例：相对指针（如 `../../main/.jj/repo`）解析出的 `-R` 实参等于期望主根路径——验证：用例通过

## 3. wizard 空集校验

- [x] 3.1 `validate_revset`（`src/main.rs:708`）改为 `--limit 1` + stdout 非空判定；空输出返回"base revset 解析为空集"类错误——验证：3.2 用例通过
- [x] 3.2 新增/更新单元测试：`none()` 被拒；`wizard_final_base_rev_uses_chain_when_untouched` / `wizard_final_base_rev_validates_edited_values` / `wizard_final_base_rev_validates_chain_values_too` 保持通过——验证：`cargo test` 全绿
- [x] 3.3 手工冒烟（真实 jj + 临时仓库）：提交 `none()` 时 wizard 停留、错误行提示空集，无 workspace 注册与目录残留——验证：python pty 驱动真实 TUI（40x120 窗口）提交 `none()`，错误行含 "no commits"、`jj workspace list` 与 workspace_root 目录均无新增，SMOKE PASS

## 4. 文档与验证

- [x] 4.1 README "Choosing the base revision" 及创建流程段落更新：base revset 在 fetch 后、主仓库上下文解析并以 commit id 落地（`workspace add -r` 与脚本 rebase 的一致性表述同步）——验证：文档与 spec/行为一致
- [x] 4.2 `cargo build --release` 成功——验证：命令退出码为零
- [x] 4.3 真实仓库端到端冒烟：base=`@` 创建无自我 rebase 报错（正确 no-op）；base=`main@origin` + 外部推进远端后落新 tip；远程分支删除后警告 + 跳过且脚本零退出——验证：三场景 PASS（附加第四场景：多 commit base 产出单一 union `-d` 且形成双父 merge）
- [x] 4.4 `openspec validate rebase-base-resolution --strict` 通过——验证：命令退出码为零
