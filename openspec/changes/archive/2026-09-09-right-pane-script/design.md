# Design: right-pane-script

## Context

现状（change #1/#2 落地后）：wizard 为右侧 pane 生成一条拼接命令 `nohup <finish> … & /bin/sh -c '<jj 物化+bookmark+fetch+rebase>'`，其中内层命令经 `sh_c_escape` 逐引号转义——这套管道的存在理由是 pane 可能运行 fish 等非 POSIX shell，`( … )` 子 shell 组无法被它们解析。代价是命令串不可读（用户在终端看到一大坨转义序列）、测试面怪异（断言"能被 fish parse"）。

脚本化后这些复杂度整体消失：脚本自身是 `#!/bin/sh`，pane 只需调用它一次，非 sh 交互 shell 解析的仅是一行普通命令。

## Goals / Non-Goals

**Goals:**

- 新增随插件分发的静态脚本 `scripts/setup-workspace.sh`，承载右侧 pane 的全部 setup 动作。
- pane 命令缩为一行脚本调用；删除 `sh_c_escape` 与 `jj_shell_words` 拼接。
- 行为逐字保留（含失败语义），假 `jj` 日志断言作为回归守卫。
- 脚本调用与进程内调用形态等价（前导参数经 `"$@"` 传播）。

**Non-Goals:**

- 不参数化 `base-rev`（仍传 `'trunk()'` 字面量，由 per-repo-base-rev change 替换为解析值）。
- 不改 `jj.command` 解析语义（#2 的解析唯一性继续成立）。
- 不引入新配置键。

## Decisions

### D1: 静态脚本随插件分发，而非每次生成

**选择**：`scripts/setup-workspace.sh` 作为仓库资产提交（可执行位 100755），`plugin install`（托管 checkout）与 `plugin link`（本地目录）天然携带。

**理由**（采纳用户提议，否决"向 `HERDR_PLUGIN_STATE_DIR` 生成唯一命名文件"的初稿）：零写入、零生成逻辑、天然无并发竞争；脚本本身可 review、可审计；用户 `cat` 即见全部行为。
**代价**：无。

### D2: 插件 exe 由脚本自定位，不作为参数

**选择**：脚本经 `$0` 推导 `PLUGIN_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)`，exe 取 `$PLUGIN_ROOT/target/release/jj-workspace`。

**依据**：相对布局由 manifest 保证——`[[build]]` 固定产出 `target/release/jj-workspace`，所有 actions 的 command 也引用 `./target/release/jj-workspace`；`plugin install`（托管 checkout）与 `plugin link` 两种模式下脚本都位于 `<root>/scripts/`。wizard 传给 pane 的脚本路径是绝对路径（来自 `HERDR_PLUGIN_ROOT`），`dirname "$0"` 稳定。

### D3: 参数签名——6 个固定参数 + 尾部可变参承载前导参数

```
setup-workspace.sh <jj路径> <base-rev> <书签名> <workspace-id> <tab-id> <pane-id> [前导参数...]
```

- `jj路径` = `ResolvedJj.executable`（#2 解析结果）；尾部可变参 = `ResolvedJj.extra_args`（argv 形态的前导参数）。
- 脚本内 `shift 6` 后以 `"$JJ_EXE" "$@" <子命令>…` 调用：空 `"$@"` 展开为零词（POSIX），string 形态退化为裸调用——**脚本调用 ≡ 进程内 `Command::new(exe).args(extras).args(subcmd)`**。
- 不用 join/eval：join 在含空格路径上碎裂；eval 引入注入面。逐元素 `shell_quote` 由 wizard 完成，pane shell 解析后交给脚本干净 argv，脚本内部零解析。
- 插件 exe 不经环境变量传递（`VAR=x cmd` 前缀非 fish 语法，会重引 shell 兼容问题）。

### D4: 脚本内部结构——`set -e` 等价于原 `&&` 链，bookmark 例外显式保留

```sh
#!/bin/sh
# 注释：各步骤说明
nohup "$EXE" finish-tab "$WS" "$TAB" "$PANE" >/dev/null 2>&1 </dev/null &
set -e
"$JJ_EXE" "$@" sparse set --clear --add .
"$JJ_EXE" "$@" bookmark create "$NAME" -r @ || printf '%s\n' 'warning: …' >&2
"$JJ_EXE" "$@" git fetch
"$JJ_EXE" "$@" rebase -s @ -d "$BASE_REV"
```

- `set -e` 等价原 `&&` 链的"失败即停"；`cmd || fallback` 语义在 `set -e` 下合法，bookmark 失败仅警告继续——两条失败语义逐字保留。
- nohup 行置于 `set -e` 之前且为后台启动，不受其影响。
- 原重定向技巧（`>/dev/null 2>&1 </dev/null &`）保留在脚本内，作用对象不变。

## Risks / Trade-offs

- [可执行位在非常规安装流程中丢失] → git 保留 100755；若丢失，pane 终端会立即显示 permission denied（可见、可定位），README 排障注一行。
- [脚本依赖相对布局 `../target/release/…`] → 与 manifest actions 的既有依赖完全一致，非新增约束；布局破坏时 actions 同样不可用。
- [`$0` 依赖绝对路径调用] → wizard 构造命令时恒以 `HERDR_PLUGIN_ROOT` 绝对路径引用脚本（spec 场景锁定）。

## Migration Plan

1. 单 commit 落地脚本 + main.rs 改造；行为不变（假 `jj` 日志断言逐行守护）。
2. 回滚 = revert 单 commit；无持久状态。

## Open Questions

（无 —— 脚本位置/签名/等价性/失败语义均已定案。）
