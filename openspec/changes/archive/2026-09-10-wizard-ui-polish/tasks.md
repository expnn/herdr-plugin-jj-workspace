# Tasks: wizard-ui-polish

实现前置：source-jj-only（B）已合并；以下均基于 B 后的 `draw_workspace_wizard` 结构（无 kind/badge/folder 变体/置灰/警告行、零候选可达）。

## 1. 弹窗几何与垂直骨架重排

- [x] 1.1 将 wizard modal 尺寸 86×22 → 86×26（`render_modal_shell` 调用处），确认 herdr overlay pane 常见高度下仍安全（`min` 钳制与高度 guard 保留）。
- [x] 1.2 按 D4 预算重构 `draw_workspace_wizard` 的行推进：header(1) + 顶部提示行(1) + source section(标题/query/列表) + 间隔行 + name section + 间隔行 + base section + 间隔行 + checkout section + 间隔行 + error(可空) + 底部按钮（锚 inner 底）。
- [x] 1.3 列表可用高度改为自适应：内区高减去固定块后 clamp(3..8)，保证小终端不崩、大屏不空荡。

## 2. 标题样式统一（D1/D7）

- [x] 2.1 新增辅助 `section_title_style(focused: bool, p)` → `BOLD` + (focused ? accent : subtext0)；checkout/只读标题直接 subtext0+BOLD。
- [x] 2.2 用该辅助替换 source/name/base 三段内联标题样式（现非聚焦 overlay0 态全部收敛为 subtext0 粗体；B 已删 base 的 surface_dim 第三态，确认无残留）。
- [x] 2.3 checkout 标题渲染为其固定值（B 后无 folder/workspace 变体，恒 "checkout"），subtext0+BOLD。

## 3. 操作提示收敛（D3）

- [x] 3.1 删除标题行内全部操作提示后缀：source 标题去掉 `type to filter · ↑/↓ navigate · tab edit name/base`，name 去掉 `tab to edit`，base 去掉 `jj revset · tab to edit`（标题保留语义名："source workspace" / "new workspace name" / "base" 或 "base · jj revset" 取舍见下）。
- [x] 3.2 header 下方渲染静态提示行（overlay0）：`type to filter or edit · ↑/↓ select · tab switch · ↵ create · esc cancel`；文案实现时定稿，确认不含多余修饰。
- [x] 3.3 冒烟确认弹窗内无第二处操作提示文字（spec：标题行不含操作提示）——由渲染测试断言 `tab to edit`/`tab edit name/base` 不存在覆盖。

## 4. 内容统一缩进与对齐（D2）

- [x] 4.1 定义内容起始列常量（相对 inner.x，取与 marker 槽兼容的 3 字符前导方案，含 1 字符 marker 位），所有内容行（列表 label、name/base 值条文本、checkout 路径）以它为基准渲染。
- [x] 4.2 列表 marker 槽恒定宽：选中 `▸ `、未选中等同宽空位；确认列表项 label 不随选中左右位移（spec Scenario "选中行文字不位移"）。
- [x] 4.3 name/base 输入值文本与 checkout 路径文本对齐到列表项 label 起始列（O1 定标：可见文本同列）；用固定常量实现，不做逐行 magic offset。

## 5. 状态渲染（D5/D6）

- [x] 5.1 零候选 draw 折叠：`choices.is_empty()` 时仅渲染 source 标题 + 空态文案（`no jj workspaces — open herdr's project picker instead`）+ esc cancel 按钮；不渲染 name/base/checkout section 与 create 按钮（覆盖 B 现状的全结构渲染）。顶部提示行同态收敛：不显示 create/tab/select 无效操作（整行替换为 esc 语义或隐藏）。
- [x] 5.2 核验（不改动）B 已提供的零候选键短路：`run_workspace_wizard` 中 `choices.is_empty()` 时 Tab/Enter no-op（main.rs 现含此守卫）；draw 折叠后冒烟确认焦点不会落到隐藏 section、无错误行、esc 可退出。
- [x] 5.3 查询无匹配分支：候选存在但 filtered 空时列表区显示 `no matching workspaces`；name/base 保持独立输入值、checkout 预览显示占位 `no matching workspace`（不派生假路径）。
- [x] 5.4 空 query 占位：焦点在 WorkspaceSearch 且 query 为空时输入条内显示浅色 `filter…`（overlay0）；焦点离开或非空时隐藏。

## 6. 验证

- [x] 6.1 `cargo build --release` 与 `cargo test` 全绿；确认字段轮换/空列表索引等既有逻辑测试仍通过（渲染改动不应触碰状态机）。
- [x] 6.2 有 TTY 时手动冒烟三态视觉：正常态（聚焦切换仅标题颜色变化、内容同列对齐、无散落提示）、零候选态（仅 source+esc）、查询无匹配态（下方 section 保留）；小终端（<24 行）高度不崩。
- [x] 6.3 README 若有按键/流程措辞与旧提示相关同步更新；`openspec validate --change wizard-ui-polish` 通过。
