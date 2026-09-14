## Why

右侧 setup 脚本第 5 步 `jj rebase -s @ -d <base-rev>` 在**新工作区的 `@` 上下文**里求值 base revset，而第 3 步 `jj workspace add -r <base-rev>` 是在**主仓库上下文**里求值的。两者的 `@` 位置相差一代，导致：base 输 `@` 时实际执行 `rebase -s @ -d @`（jj 实测 `Error: Cannot rebase <commit> onto itself`，exit 1，末步失败）；base 输 `@--` 等 `@` 相对表达式时 rebase 静默落到错误 commit（实测 exit 0，比报错更危险）。此外 wizard 现有 revset 校验只验证"可解析"，空集 revset（如 `none()`）能通过提交，随后 `workspace add -r 'none()'` 先注册工作区再报 `Error: Empty revision set`，留下半成品（实测：workspace 已列入 `workspace list`、目录只剩 `.jj`）。

## What Changes

- setup 脚本第 5 步重写为"主仓库上下文重解析 + 按 commit id rebase"：脚本经副工作区 `.jj/repo` 指针推导主仓库根（脚本签名不变、无新参数），在 fetch 之后以 `jj -R <主仓库根> --ignore-working-copy log -r <base-rev> -T 'commit_id ++ "\n"'`（每 commit 一行）在**主上下文**重解析，脚本将多行结果连接为 ` | ` 分隔的单一 union revset；结果非空则执行 `rebase -s @ -d <union>`。
- 解析调用非零退出或解析结果为空 → stderr 警告并**跳过 rebase**（脚本继续，不因此非零退出）；rebase 自身失败仍非零退出（既有失败语义的其余部分不变）。
- wizard 提交校验从"revset 可解析"升级为"可解析且至少解析出 1 个 commit"：空集以错误行拒绝、停留 wizard 可继续编辑（`jj log -r <expr> --no-graph --limit 1` + stdout 非空判定）。
- `jj git fetch` 的位置与形态不变（仍在副工作区、仍在 rebase 之前）。

非 BREAKING：默认 `trunk()` 的既有语义（基于 fetch 后最新）逐字不变；`@` 相对 base 从"报错/静默错位"修正为正确结果；空集 base 从"半成品失败"提前为 wizard 拒绝。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `workspace-setup-script`: 调用形态新增"主仓库根推导 + 主上下文解析调用"；rebase 目标改为解析出的 commit id；失败语义新增"解析失败/空集 → 警告并跳过 rebase"；string/argv 形态场景的调用日志更新。
- `workspace-wizard`: revset 提交预校验新增空集拒绝（要求 ≥1 个 commit）。
- `plugin-config`: 解析链落地语义更新——同一 revset 仍同时驱动 `workspace add -r` 与脚本，但脚本侧改为 fetch 后在主仓库上下文重解析并 rebase 到 commit id；含空格/引号 revset 的安全传递场景更新。

## Impact

- **代码**：`scripts/setup-workspace.sh`（新增主根推导与解析调用、rebase 参数改为解析结果；其余步骤不变）；`src/main.rs`（wizard 提交校验：`--limit 0` 判定改为 `--limit 1` + stdout 非空；无接口/签名变化）。
- **测试**：setup 脚本假 jj 用例（主根指针 fixture、调用日志含 `-R <主根> --ignore-working-copy log`、union id 拼接、解析失败/空集降级路径）；wizard 校验用例（`none()` 被拒、正常 revset 照常通过）。
- **spec**：`workspace-setup-script`、`workspace-wizard`、`plugin-config` 三个 delta。
- **不受影响**：`workspace add` / sparse 物化 / bookmark / fetch 步骤；插件配置 schema；herdr 集成。
