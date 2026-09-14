# Proposal: remove-workspace-dialog

## Why

`remove` 目前是 headless 的一刀切：要么 toast 拒绝，要么直接执行 `jj workspace forget` + 删除目录 + 关闭整个 tab。由此带来三类问题：

1. **无审阅**：用户看不到将执行的任务（迁移 / forget / 删目录 / 关 pane）；脏工作副本只有事后一条 toast，缺少出路指引。
2. **关 tab 过激**：删除目录实际只影响 cwd 位于该目录内的 pane，但现状关闭整个 tab——用户在 tab 里后建的、cwd 已在别处的 pane（可能跑着重要命令）会被连带杀死。
3. **可达性缺陷**：目标解析要求聚焦 pane 的 cwd 恰好是 workspace 根（cd 进子目录即误报 "not a jj workspace"）；在主 workspace 触发只能 toast 拒绝；插件自己创建的副 workspace（含目录已手删的 stale 注册）无法从主仓库侧发现和清理。

## What Changes

- **删除改为对话框流程**：`remove` action 只做预检（config/`jj.command` 解析、从聚焦 pane cwd 上溯 jj workspace 根、非 jj/不安全路径判定），随后打开 overlay 对话框 pane（新增 `remove-wizard` entrypoint，与创建 wizard 同族）；确认后由对话框进程按序执行删除。
- **BREAKING（交互变更）**：`remove` 不再是单步执行——需在对话框中授权；不再自动关闭整个 tab，改为只关闭勾选的 pane（tab/workspace 是否关闭由 herdr「最后一个 pane 关闭即关 tab」级联决定）。
- **主 workspace → 副 workspace picker**：在主 workspace 触发时列出该仓库全部副 workspace（含 `(missing on disk)` 的 stale 注册，可仅 forget），选定后进入删除审阅。
- **Plan 审阅 + 选择**：
  - `1 migrate opencode sessions` —— 列出绑定 session（title / 相对目录 / 时间）并逐条勾选（默认全选；取消 = 不迁移）；
  - `2 jj workspace forget` —— 说明 commits/bookmarks 留在共享存储；
  - `3 delete directory` —— 显示完整路径（stale 时 already missing，执行时跳过）；
  - `4 close panes` —— 全局扫描 cwd 或 foreground_cwd 位于目标目录内的 pane，按 herdr workspace → tab → pane 层级展示并勾选（默认全选）。
- **Checks 与修复指引**：工作副本干净、opencode DB 可读两项检查；脏时逐字展示 `jj commit -m "<message>"` / `jj restore` 出路，其中 commit 可在对话框内输入 message 授权执行，restore 仅展示、永不代执行。
- **执行与 Status**：`↵` 授权后复检 clean → 迁移 → forget → 删目录 → 关 pane（顺序即安全边界）；对话框内 Status 视图逐项推进，成功自动退出（自身 pane 随之关闭），失败停留并给出 `error.log` 指针。
- **opencode 迁移扩展**：新增只读 inspect（预览 session 行）与按选中子集迁移；上报出口由 toast 改为对话框 Status。

## Capabilities

### New Capabilities

- `workspace-removal-dialog`: 删除对话框的交互契约——入口与模式（picker/review/status）、Plan/Checks 的渲染与层级列表、选择默认值与键位、阻断指引与授权修复、状态视图与失败呈现、与创建 wizard 的视觉一致性。

### Modified Capabilities

- `workspace-removal`: 目标解析改为聚焦 pane cwd 上溯；主 workspace 走 picker 而非拒绝；脏检查升级为对话框阻断项（含确认时复检）；新增按 pane 选择关闭（替代 tab 关闭）与执行顺序/失败语义；stale 目标处理。
- `opencode-session-migration`: 新增只读预览（inspect）与按选中子集迁移；上报出口改为对话框 Status。

## Impact

- `herdr-plugin.toml`：新增 `[[panes]] remove-wizard`（overlay）。
- `src/main.rs`：`cmd_remove` 拆分为预检 + 打开对话框 pane；新增 `remove-wizard` 入口与三态 TUI；jj workspace 枚举、pane 扫描/关闭（含自 pane 排除）、执行管线。
- `src/opencode_migration.rs`：`inspect` 只读预览 + 子集迁移。
- `README.md`：Removing a workspace 与 Troubleshooting 章节重写。
- 无新配置键；不再使用 `herdr tab close`。
- 本机真实冒烟依赖运行中的 herdr 0.8.2 server（使用从 `/proc` 恢复的 0.8.2 二进制；server 不可重启）。
