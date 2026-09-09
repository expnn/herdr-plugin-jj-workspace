# Proposal: per-repo-base-rev

## Why

`jj.base_rev` 是插件全局单值，但"新工作区基于哪个 revision"本质是仓库级知识——同一台机器上有的仓库要基于 `main@origin`，有的要基于 `dev@origin`。同时存在既有 quirk：右侧 pane 的 rebase 目标硬编码 `'trunk()'`，与 `jj.base_rev` 自相矛盾（在 dev 上建工作区又被 rebase 回 trunk）。本 change 把 base_rev 的决策权下放到仓库级（jj config），并让 rebase 目标与 base_rev 同源，闭环 quirk。

## What Changes

- **base_rev 解析链**：`jj config get herdr.base-rev`（cwd = 选定 source 仓库；非零退出视为未设置）> config.toml `jj.base_rev` > 内置默认 `trunk()`。
- **wizard 新增第三字段 base**：默认值为解析链结果，可编辑，支持任意 `jj` revset 表达式；选中非 jj 源时置灰并提示忽略；Enter 提交时经 `jj log -r <expr> --no-graph --limit 0 --no-pager` 预校验，失败用 jj 原生错误信息留在 wizard。解析采用 dirty 标志惰性策略：字段未被编辑则按最终选定的 source 在提交时现算，被编辑则直接校验用户输入。
- **rebase 目标参数化**（闭环 quirk）：右侧 setup 脚本收到解析后的 revset 作为 `$2`，`rebase -s @ -d "$2"`；`jj git fetch` 保持全量拉取不变。
- `jj config get` 仅在仓库上下文可用（spike 已验证 0.45.1：合并 repo/user 层、副工作区可读、`-R` 可用；仓库级配置物理位于 `~/.config/jj/repos/<hash>/`，直接读文件不可行）。

## Capabilities

### New Capabilities

- `workspace-wizard`: wizard TUI 的字段行为——base 字段的默认值来源、编辑、置灰、revset 提交预校验与 dirty 惰性解析。

### Modified Capabilities

- `plugin-config`: 新增 `jj.base_rev` 的解析链需求（jj config 仓库级覆盖 > 全局配置 > 默认值）。

## Impact

- `src/main.rs`：`resolve_base_rev(config, repo)` 解析链；wizard TUI 三字段化（`WizardField::Base`、置灰、校验）；脚本调用第二参数从 `'trunk()'` 换为解析值。
- `scripts/setup-workspace.sh`：无结构变化（`$2` 语义从字面量变为任意 revset，`"$2"` 引用已安全）。
- README：配置层级说明（三种配置场景示例）+ base 字段说明。
- 依赖：`right-pane-script` change 先行（脚本签名已含 `$2` 参数位）。
