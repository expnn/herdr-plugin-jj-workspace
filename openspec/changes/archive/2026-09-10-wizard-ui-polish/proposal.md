# Proposal: wizard-ui-polish

## Why

wizard 当前把**操作提示混在 section 标题同一行**（如 ` source workspace  type to filter · ↑/↓ navigate · tab edit name/base`、`name  tab to edit`、`base  jj revset · tab to edit`），逻辑错位：提示属键盘操作、不属标题；且 `tab to edit` 措辞误导——tab 实际是在 source/name/base **三个字段间轮换**，不是"编辑本字段"。标题样式随焦点分裂（聚焦 accent 加粗、非聚焦灰色），用户观感不一致。source-jj-only（B）落地后 wizard 成为纯 jj 三字段 + checkout 只读预览 + 空态可达，布局可收敛为稳定文法：**每 section = 标题 + 缩进内容**。

## What Changes

- **section 化布局**：wizard 渲染为统一结构的 section（Source Workspace / New Workspace Name / Base / Checkout），每 section = 独立标题行 + 缩进内容行；section 间留一个空白行。checkout 为只读 section（值恒为派生路径）。
- **标题样式统一**：标题恒粗体；聚焦 section 标题用 accent（蓝），非聚焦用 subtext0；checkout 只读、永不聚焦 → 恒 subtext0 粗体。焦点只通过标题颜色表达（不再同时切换粗体/灰色）。
- **操作提示集中为顶部单行静态提示**：modal 头部下方渲染 `type to filter or edit · ↑/↓ select · tab switch · ↵ create · esc cancel`（overlay0）；删除标题行内全部操作提示文字。
- **内容统一缩进**：source 列表、name/base 输入值、checkout 路径相对标题缩进一致（固定列）；列表选中 marker `▸` 与内容共用同一前导槽位（选中显 `▸`、未选中留同宽空格），文字不随选中位移；name/base/checkout 值与列表项 label 首列对齐。
- **弹窗几何**：86×26（内区 +2 行容纳标题独立成行与 section 间隔）；当前渲染函数按新行数重排。
- **零候选折叠渲染**（零候选可达与键短路由 B 提供）：B 已实现——候选为空时 wizard 照常打开、source 列表区显示空态文案（`no jj workspaces — open herdr's project picker instead`）、run 循环内 Tab/Enter 在 `choices.is_empty()` 时短路（不切字段、不报错）。本 change 在其上做 **draw 层折叠**：零候选时只渲染 source 标题 + 空态文案 + esc cancel 按钮，**不渲染** name/base/checkout section 与 create 按钮，顶部提示行同步收敛（不显示 create/tab/select 等无效操作）。
- **查询无匹配保留结构**：候选存在但 query 过滤空时，列表区显示 "no matching workspaces"；下方 section 保留——name/base 保持各自独立输入值，checkout 预览显示占位（无可选中源派生路径）。
- **过滤输入条占位**：query 为空且搜索聚焦时，输入条内显示浅色占位 `filter…`。
- tab 循环语义不变（source/name/base），仅移除字段内 "tab to edit" 相关误导提示。
- 本 change 纯渲染层：不改变字段行为、校验、提交语义与按键绑定；零候选的键短路已由 B 实现，A 只改 `draw_workspace_wizard` 及渲染辅助。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `workspace-wizard`: wizard TUI 的渲染结构需求——section 化布局、标题样式统一、顶部单一提示行、内容统一缩进、空态/占位渲染。依赖 source-jj-only（B）定案后的世界（无 badge/kind、无 folder 预览变体、无置灰态、无 warning 行、零候选可达）。

## Impact

- `src/main.rs`：`draw_workspace_wizard` 及其渲染辅助重排（标题行文案去提示、内容行统一缩进、几何 86×26、零候选分支、占位文字、间距）；仅此函数及其被调辅助。
- 测试：无 TUI 渲染单测；相关逻辑测试（字段轮换、空列表索引）保持通过；`cargo build --release` + `cargo test` 全绿；有 TTY 时手动冒烟（正常态/零候选/查询无匹配/聚焦切换的视觉确认）。
- README：wizard 流程描述若涉及按键提示同步措辞（若有）。
- 依赖：source-jj-only（B）先落地；本 change 不引入新配置键、不改按键绑定。
