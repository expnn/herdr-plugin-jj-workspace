## MODIFIED Requirements

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

## ADDED Requirements

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
