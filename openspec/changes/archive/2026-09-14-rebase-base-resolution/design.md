## Context

见 proposal.md —— Why。塑造本方案的关键现状与约束：

- setup 脚本是随插件分发的静态文件，所有 jj 调用以 `"$JJ_EXE" "$@" <子命令>` 形态执行（`"$@"` 承载 argv 形态 `jj.command` 的前导参数）；脚本签名已固定为 6 固定参数 + 尾部可变参数。
- 副工作区的 `.jj/repo` 是指向主仓库 store 的**相对路径指针**；插件进程内的 `repo_root()`（`src/main.rs:1976`）已使用同一机制，wizard 第 3 步 `workspace add -r` 的 cwd 即由它推导。
- fetch 在副工作区、异步于右 pane，这是"本地状态快速创建 → 异步网络更新"两段式设计（提交 `2da97da`）的后半段；第 5 步 rebase 的职责是为**移动 revset**（`trunk()`、`x@origin`）提供 fetch 后新鲜度，对固定 revset 是无害 no-op。
- 本 change 前的缺陷（proposal 已述）源于第 5 步在**新工作区的 `@` 上下文**对 base revset 二次求值。

本次讨论中新增的 jj 0.45.1 spike 证据（决策依据）：

| 实验 | 结果 |
|---|---|
| `rebase -s @ -d @` | `Error: Cannot rebase <commit> onto itself`，exit 1 |
| `rebase -s @ -d '@--'`（定位在新 ws 上下文） | exit 0，@ 被静默改基到错误 commit（主上下文应为 c1，实际落到 root） |
| 主仓库单独移动 | 副 ws 内一切 jj 命令瘫痪（`Cannot access ../../…`）——指针可用性 = jj 可用性 |
| 整棵树移动 | 指针照常（相对路径重锚）；`workspace root --name default` 也返回新路径 |
| 主 ws 改名 | `jj workspace root --name default` → `Error: No such workspace: default`（仓库健康、指针仍可用） |
| `rebase -o/--onto`（别名 `-d/--destination`）重复传入 | exit 0，产生多父（merge） |
| 单一 union revset `-d "A \| B"` | 与重复 `-d` 等价，exit 0 |
| 模板 `separate(" | ", commit_id)` / `commit_id ++ "\n"` | 单 commit 样本曾显示 separate 可用；实现期 4-commit 样本证伪——jj 0.45.1 的 log 模板按 revision 逐个求值，separate 多 commit 输出为无分隔符拼接（`cat -A` 实测）；`commit_id ++ "\n"` 逐行输出可靠，空集输出为空、exit 0 |
| 远程分支删除后 fetch | `<branch>@origin`→`[deleted] untracked`，解析 `Error: Revision ... doesn't exist`，exit 1 |
| `log -r 'none()' --limit 1` | exit 0、stdout 空；`workspace add -r 'none()'` 先注册工作区后 `Error: Empty revision set`，留下半成品 |

## Goals / Non-Goals

**Goals:**

- 第 5 步 rebase 的 base 求值上下文与第 3 步 `workspace add -r` 完全一致（主仓库上下文），消除 `@` 类表达式的报错与静默错位。
- 保留第 5 步的唯一价值：移动 revset 的 fetch 后新鲜度；默认 `trunk()` 的既有行为不变。
- 空集 base revset 在 wizard 提交阶段拒绝，不再产生"已注册 + 半成品目录"的失败现场。
- 脚本外部契约不变：6 固定参数 + 尾部前导参数。

**Non-Goals:**

- 不改变 fetch 的位置（仍副工作区）与形态（全量拉取）。
- 不改变创建步骤 ③ `workspace add -r` 的本地解析语义、⑤ sparse 物化、⑥ bookmark。
- 不对用户输入的 revset 做任何表达式改写；原样传递、原样求值。
- 不引入新的脚本参数，也不使用 `jj workspace root --name default` 定位主仓库根（理由见决策 1）。

## Decisions

### 决策 1：主仓库根由脚本内 `.jj/repo` 指针推导

脚本读取当前目录（副工作区）的 `.jj/repo`：指针相对 `.jj/` 解析、绝对路径原样使用，结果规范化（镜像 `repo_root()`）。脚本签名不变。

**备选与否决理由**：

- `jj workspace root --name default`（改动最小，约 2 行）：实测在主 ws 改名后失败（仓库完全健康）——这是唯一"其他方案可用而它不可用"的场景，且引入了插件其他部分不存在的"主 ws 名恰为 `default`"假设。
- 插件传参（第 7 固定参数）：最显式、零重复，但与"最小外部契约变化"冲突：需要动 `open_tab_layout`/`setup_script_command` 签名、spec 接口与测试。
- 实测总结：主仓库单独移动 → 各方案同失败（jj 自身瘫痪）；整棵树移动 → 指针与 name 都存活；主 ws 改名 → 仅 name 失败。**指针的失败集合等于 jj 自身的失败集合**，且指针正是 wizard 刚刚用过的机制（③ 的 cwd 由其推导），不是新增依赖。

### 决策 2：fetch 后在主仓库上下文重解析，rebase 目标为 commit id(s)

解析调用：`jj -R <主根> --ignore-working-copy log -r "$BASE_REV" --no-graph -T 'commit_id ++ "\n"'`（每 commit 一行）；脚本将行连接为 ` | ` 分隔的单一 union，非空时 `rebase -s @ -d "<union>"`。

- 与 ③ 同上下文，是本 change 的修复本体。
- 放在 fetch 之后，保留"落到最新"的语义（对移动 revset）。
- `--ignore-working-copy`：纯读、不在主仓库留下 snapshot 副作用，读到的是 ③ 时点附近的 wc 状态。

**放弃的备选**：wizard 解析候选 id 传参 + 解析为空时回退候选。分析结论：候选 ≡ 副 ws 的 `@-`（W 的父，由 `workspace add -r` 构造），"回退到候选"与"跳过 rebase"行为等价（rebase 到 `@-` 是 no-op），而跳过更安全——若左 pane agent 期间自行改过 `@`，不会被强行拉回。因此无需新增参数。

### 决策 3：多 commit 结果用单一 union revset（而非重复 `-d`）

重复 `-d` 与单一 union revset 均实测可行且等价（都产生 merge parents）。选择 union：单一带引号参数，与既有 `-d "$BASE_REV"` 形态一致，sh 侧无需无引号展开。实现期修正：连接不能在 jj 模板内完成——`separate(" | ", commit_id)` 按 revision 逐个求值，多 commit 时输出为无分隔符拼接（4-commit 样本 `cat -A` 实测，证伪了设计期的单样本结论）——故采用 `commit_id ++ "\n"` 逐行输出，脚本侧以 POSIX awk 连接（`paste -sd ' | '` 亦被否决：serial 模式仅使用第一个分隔符字符）。

### 决策 4：解析失败或空集 → stderr 警告 + 跳过 rebase（脚本零退出）

解析失败是真实可达形态（远程分支被删后 `Revision ... doesn't exist`，exit 1），不只是理论空集。该步只影响落点新鲜度，不值得让整个 setup 失败；警告写在右 pane stderr（可见）。rebase 自身失败仍非零退出（既有语义保留）。

### 决策 5：wizard 校验升级为"可解析且 ≥1 commit"

`jj log -r <expr> --no-graph --limit 1 --no-pager`：非零退出沿用 jj 原生错误；退出为零但 stdout 为空 → 自定义错误（提示 base revset 解析为空集），停留 wizard。`--limit 1` 同时保留原有解析错误检测并避免大 revset 的输出开销。这消灭了 `workspace add -r 'none()'` 的半成品现场（实测：workspace 已注册、目录残留、exit 1）。

### 决策 6：fetch 保持原位（副工作区）

fetch 更新共享 store，主/副工作区执行等价（双向实测）。保留在副 ws 使改动最小，⑤⑥⑦ 调用序列与前半段失败语义完全不动。

## Risks / Trade-offs

- [脚本内 sh 指针逻辑与 Rust `repo_root()` 语义漂移] → spec 固化三种形态（相对/绝对/不可读）的语义；测试以 fixture 覆盖相对指针；两处同仓库演进，漂移窗口极小。
- [base revset 解析出大量 commit 时 union 参数很长] → 与用户显式 revset 的语义一致（`rebase -d` 本就接受多目标），不设上限。
- [降级路径可能掩盖真实故障] → 警告固定输出到右 pane stderr；只跳过 rebase，其余步骤照常，失败面收窄而非扩大。
- [新增的 wizard 空集错误文案需要测试断言] → spec 只约束"明确错误信息"，文案由实现选定并在测试中固定。
- [`.jj/repo` 不可读的极端情况] → 与解析失败同路降级（警告 + 跳过）；正常路径上该文件必然可读（③ 刚刚依赖过同一指针）。
