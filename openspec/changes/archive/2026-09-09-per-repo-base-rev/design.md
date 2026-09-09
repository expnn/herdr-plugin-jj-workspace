# Design: per-repo-base-rev

## Context

change #1 的 `jj.base_rev` 是全局单值；#2 的解析唯一性已确立"一次解析处处使用"。两个前置事实（spike 实测，jj 0.45.1，已存项目记忆 #130）：

1. `jj config get <key>` 读合并配置（repo > user 层），原样输出值、exit 0；键缺失 exit 1 + stderr。副工作区（`.jj/repo` 文件指针）与 `jj -R <path>` 均可读。
2. 仓库级配置物理位于 `~/.config/jj/repos/<hash>/config.toml`（机器本地，**不在** `.jj/repo/config.toml`）——直接读文件不可行，`jj config get` 是唯一接口。

既有 quirk（项目记忆 #128）：右侧 rebase 目标硬编码 `'trunk()'`，与 `jj.base_rev` 矛盾。`right-pane-script` change 已为脚本留出 `$2` 参数位。

## Goals / Non-Goals

**Goals:**

- base_rev 解析链：仓库级（jj config）> 插件全局（config.toml）> `trunk()`。
- wizard base 字段：解析链默认值 + 可编辑 revset + 提交预校验 + 非 jj 源置灰。
- rebase 目标与 base_rev 同源（quirk 闭环）；`jj git fetch` 行为不变。

**Non-Goals:**

- 不做 `bootstrap_paths` 等其他键的仓库级覆盖（agent 属性 ≠ 仓库知识）。
- 不做定向 fetch（jj fetch 为全量书签拉取，按 revset 定向无收益）。
- 不改变 `jj.command` 解析语义。

## Decisions

### D1: 解析链与优先级

```
resolve_base_rev(config, repo):
  1. jj config get herdr.base-rev   (cwd = repo；exit ≠ 0 → 未设置)
  2. config.toml [jj] base_rev
  3. "trunk()"
```

- `jj config get` 非零退出一律视为未设置：不区分"键缺失"与"jj 故障"——jj 若损坏，下一步 `workspace add` 会以 jj 自己的错误暴露。
- **jj 用户级配置也视为有效答案**：`config get` 合并 repo/user 层，用户可用 `--user` 设个人全局默认。文档写明三种配置场景：`jj config set --repo herdr.base-rev dev@origin`（per-repo）、`--user`（个人全局）、config.toml（插件全局）。
- 键名 `herdr.base-rev`（jj config 惯用中划线小写）。

### D2: wizard base 字段与 dirty 惰性解析

**选择**：第三字段 `Base`，Tab 循环 `WorkspaceSearch → Name → Base`。字段显示解析链对**初始 source** 的预填值；解析的最终执行在提交时：

- 字段未被编辑（dirty = false）→ 提交时按**最终选中**的 source 现算解析链（一次 `jj config get`，~30ms）；
- 字段被编辑（dirty = true）→ 直接使用输入值，仅做 revset 校验。

**理由**：逐键重算（Up/Down/过滤每动一次 spawn 子进程）太吵；打开时定死则在用户切换 source 后用到错误仓库的答案。dirty 标志是两者间最诚实且安静的折中。
**替代方案**：字段完全隐藏默认、提交时必现算——被否，用户失去"看见默认值"的知情权。

### D3: revset 提交预校验

**选择**：Enter 提交时（仅 jj 源且 base 字段参与）执行 `jj log -r <expr> --no-graph --limit 0 --no-pager`（cwd = source 仓库）；非零退出 → jj 原生错误（含语法错误定位）写入 wizard 现有错误行，停留 wizard，用户可编辑 base 字段后重新提交。**对最终值统一校验（无论来自解析链还是手输）**——早期版本曾豁免未编辑的解析链值（"天然有效"），被验收实测证伪：仓库级 `herdr.base-rev` 可设为不存在的 revision，该错误原会推迟到 `jj workspace add` 才暴露；现解析链值同样过预校验。

**依据**（spike）：0.45.1 无 `jj revset parse`；`jj log -n 0` 在解析 revset 后才走模板，空输出零 pager（`--no-pager` 实测被接受）；语法错误与未知符号两类失败均被捕获。
**安全性**：TUI 内同步 spawn 子进程，耗时 ~百毫秒级，事件循环可接受。

### D4: 非 jj 源置灰

**选择**：字段恒可见、恒可 Tab 聚焦，但选中非 jj source 时以 dim 样式渲染并在现有黄色警告行追加"非 jj 源忽略此项"；提交时非 jj 源跳过 base 校验与传递。

**理由**：动态隐藏会让 TUI 布局随选中项变化（高度计算、Tab 循环分支），实现与测试成本高；置灰以一行样式代码换同等信息量。

### D5: revset 到脚本的传递

wizard 提交后 base revset 作为脚本 `$2`：wizard 侧 `shell_quote(expr)` 进 pane 命令，脚本内 `"$2"` 双引号引用——revset 含空格/引号均安全（与书签名同管道）。`workspace add -r <expr>` 走 `Command` argv 直传，无转义问题。

## Risks / Trade-offs

- [`jj config get` 每次 wizard 多一次子进程] → 仅一次且惰性，~30ms，可忽略。
- [用户级 jj 配置与插件全局配置的优先级混淆] → 文档明确：jj config 中任何层的 `herdr.base-rev` 一旦设置即生效，优先于 config.toml；不想被覆盖就删掉 jj config 中的键。
- [空 revset（如 `none()`）通过校验但 `workspace add` 失败] → 极端边角；jj 原生错误信息在 fail() 路径可见，不为它加专门校验。
- [wizard 提交校验与实际创建之间状态漂移] → 窗口秒级，jj 原生错误兜底。

## Migration Plan

1. 依赖 right-pane-script 落地（脚本 `$2` 参数位已存在，当前传 `'trunk()'`）。
2. 本 change 将 `$2` 换为解析值；未配置任何 base-rev 的用户行为逐字节不变（解析链终点 = trunk()）。
3. 回滚 = revert；无持久状态。

## Open Questions

（无 —— 键名、解析链、dirty 策略、校验命令、置灰、脚本传递均已定案。）
