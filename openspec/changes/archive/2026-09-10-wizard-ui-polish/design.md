# Design: wizard-ui-polish

## Context

wizard 是插件里唯一的交互 TUI（`run_workspace_wizard` → `draw_workspace_wizard`，src/main.rs），弹窗 86×22（内区 84×20），全宽度从左到右平铺，section 标题与操作提示混排在同一行。source-jj-only（B）已合并，post-B 事实：

- 候选恒为 jj workspace 根（`load_workspace_choices_with` 祖先归一 + 过滤）；列表行 = marker + label + path，**无 `[jj]` badge/kind**；无 `source_warning`；checkout 预览恒 "Checkout"（无 folder/workspace 变体）；base 无 `surface_dim` 置灰态。
- 零候选可达：wizard 照常打开，列表区显示空态文案（draw）；run 循环 Tab/Enter 在 `choices.is_empty()` 时短路（main.rs:1386/1417，不切字段、不报错），仅 esc 退出。**但 draw 层在零候选下仍渲染 name/base/checkout section 与 create 按钮**（main.rs:1694-1805 仅替换列表区内容）。
- 查询无匹配（choices 非空、filtered 空）：列表区 "no matching workspaces"，下方 section 保留，checkout 预览显示占位 `no matching workspace`（无可选中源派生路径，main.rs:1744）。

渲染结构现状（待 A 重排）：
- 行 1：modal header "new workspace"
- 行 2：` source workspace  type to filter · ↑/↓ navigate · tab edit name/base`（标题+提示混排，焦点 accent/bold 否则 overlay0）
- 行 3：query 输入条（surface0 底）
- 行 4-10：候选列表（marker 槽 3 列恒定）
- 行 11-12：`name  tab to edit` + 值条
- 行 13-14：`base  jj revset · tab to edit` + 值条
- 行 15-16：checkout 预览 label + 值
- 行 17：error（无 warning，B 已删）
- 底行：buttons（`button_rects` 锚定 inner 底部）

标题聚焦样式在三段内联计算（source/name/base 各自 if/else），非聚焦用 overlay0，与"统一粗体 + 聚焦仅颜色"的目标不符。

## Goals / Non-Goals

**Goals:**
- wizard 内容呈现为统一 section 文法：标题恒粗体、聚焦仅颜色（accent vs subtext0）、标题行不含操作提示、内容统一缩进、section 间留白。
- 操作提示收敛到顶部单行静态文本。
- 弹窗几何 86×26，内容垂直重排不越界；零候选/查询无匹配/聚焦切换三态渲染正确。

**Non-Goals:**
- 不改字段行为、base/name 校验、提交语义、tab 循环顺序、按键绑定、配置（含零候选的 Tab/Enter 键短路——B 已在 run 循环实现，本 change 不改动）。
- 不做功能增删（B 已覆盖 jj-only 与零候选可达；本 change 仅把零候选的**渲染**从"全结构 + 列表区空态文案"折叠为"仅 source + esc"，是 draw 层变化，非新状态语义）。
- 不触碰 `run_workspace_wizard` 的事件循环与状态机，只改 `draw_workspace_wizard` 及渲染辅助。
- 不美化其他 modal（config-error 等）——保持一致性前提下最小作用面。

## Decisions

### D1: 标题文法 = 恒粗体 + 聚焦颜色差

**选择**：所有 section 标题（source/name/base/checkout）恒 `BOLD`；聚焦的 section 标题 `fg(accent)`，非聚焦 `fg(subtext0)`；checkout 只读、不在 tab 循环内，恒 `fg(subtext0)+BOLD`。移除 overlay0 灰标题态与 base 的 `surface_dim` 第三态。

**理由**：用户确认聚焦的 accent/bold 设计本身够好，唯一不一致是"非聚焦标题灰且非粗体"；统一粗体后用颜色表达焦点，视觉层级稳定且焦点仍一目了然。subtext0（166,173,200）比 overlay0（108,112,134）亮，作为非聚焦标题比现状灰更有存在感、与正文 text 形成两层。

**备选**：非聚焦用 `text`（无层次感，会与 modal header 撞车）；非聚焦维持 overlay0（正是当前被诟病的"第三个 section 标题变灰变弱"来源）。拒绝两者。

### D2: 内容统一缩进 + marker 槽对齐

**选择**：每个 section 的内容行（列表项、name/base 值条、checkout 路径）统一从固定缩进列 `INDENT`（建议 2 空格 + marker 槽宽 = 取 3 字符宽前导，含 1 字符 marker 位）开始；列表行 marker 槽恒定 3 宽（选中 `▸ `、未选中两个空格），label 从同一列起——文字零位移。name/base 值文本与 checkout 路径文本对齐到**列表 label 的起始列**（而非条带左缘）。

**理由**：当前列表 marker 槽（`▸ `/`   `，3 列）已保证选中/未选中行文字不抖，符合用户"marker 视觉不产生额外缩进"的接受度；把 name/base/checkout 内容对齐到与列表 label 同列，消除"section 内容参差"观感。

**实现提示**：用一个 `content_col = inner.x + INDENT` 常量供所有 section 内容行共用；列表行在 content_col 内先写 marker 槽再写 label；值条文本起点 = content_col + marker_槽宽? 需要实现时定标（对齐目标：name/base 值的**可见文本**与列表 label 同列，或与 marker 槽后 label 列一致——取 label 起始列，marker 仅在列表区有）。

### D3: 操作提示收敛到顶部单行静态

**选择**：modal header 正下方（原行 2 位置）渲染一行 overlay0 静态提示：`type to filter or edit · ↑/↓ select · tab switch · ↵ create · esc cancel`。所有标题行删去操作提示后缀（`tab edit name/base`、`tab to edit`）；checkout label 不再带 "folder/workspace" 变体（B 已删），恒 "Checkout"。

**理由**：用户选定"静态总览"。提示位于"开始位置"，进弹窗即见，不随焦点闪变；一次讲清四类键（输入/选择/切换/提交取消）。

**备选**：焦点上下文提示（内容随字段变）更精准但闪变、实现重，用户已否决。

### D4: 弹窗几何 86×26

**选择**：modal 高 22→26（内区 20→24），宽保持 86。垂直预算：
- header 1 + 提示行 1
- source section：标题 1 + query 条 1 + 列表 H 行（H 随 filtered 变化，上限由预算定）
- 空行 1（section 间隔）
- name section：标题 1 + 值条 1
- 空行 1
- base section：标题 1 + 值条 1
- 空行 1
- checkout section：标题 1 + 值 1
- 空行 1 + error 行（可空）+ button 行（锚 inner 底部）

列表可用高度 = 内区高 - 固定块高，clamp 到 (3..8) 保证小终端不崩。error 行与按钮行共存时不挤压（内区 24 足够）。

**理由**：标题独立成行 + section 间隔 + 统一缩进需要比现状多约 4-6 行；26 在 herdr overlay pane（常规终端 ≥24 行高）内安全。宽度不变避免 checkout 长路径视觉截断更差（路径超宽本就截断，不属本 change）。

### D5: 空态与查询无匹配的渲染分支

**选择**：
- **零候选**（`choices.is_empty()`）：B 已保证可达 + run 循环键短路；本 change 在其上把 draw 从"全结构 + 列表区空态文案"折叠为——只渲染 source 标题 + 空态文案（`no jj workspaces — open herdr's project picker instead`，overlay0）+ esc cancel 按钮；**不渲染** name/base/checkout section 与 create 按钮（对 B 现有 draw 的覆盖：B 仍渲染全结构，见 Context）。顶部提示行随之收敛：零候选下不显示 create/tab/select 等无效操作（可整行替换为 esc 语义或隐藏），避免"提示可 create 但无按钮"的矛盾。
- **查询无匹配**（候选存在但 filtered 空）：列表区显示 `no matching workspaces`（沿用现状文案，overlay0），下方 section 保留——name/base 保持各自独立输入值，checkout 预览显示占位 `no matching workspace`（无选中源可派生路径；对齐 B 实现 main.rs:1744）。

**理由**：零候选时下方字段无意义（无 source 可建 workspace），展示反而误导；用户选定"隐藏下方 section、仅 esc"。查询无匹配是过滤中间态：name/base 输入值是用户独立编辑态、保留避免丢字，checkout 依赖选中源故显示占位而非假路径。

### D6: query 空占位

**选择**：query 为空且 `field == WorkspaceSearch` 时，输入条内显示浅色占位 `filter…`（overlay0），聚焦态可见；非聚焦或已有输入时不显示。

**理由**：现状空 query 时输入条是全空 surface0 条，用户可能不知道那里可输入；占位把"这里可过滤"讲清，且用户已选定。

### D7: 聚焦样式实现收敛

**选择**：把三段内联标题样式计算收敛为一个辅助（如 `section_title_style(field, this_section, p)`），返回 `BOLD + (聚焦 ? accent : subtext0)`；checkout/read-only 场景直接 subtext0+BOLD。消除 base 的 `surface_dim` 非 jj 残留（B 后无此态）。

## Risks / Trade-offs

- [小终端高度不足] 26 高弹窗在 <24 行 pane 中可能贴边 → `render_modal_shell` 已对 `w/h` 做 `min(area…)` 钳制、`draw` 有高度 guard（现状 `inner.height < 14` return）；保持 clamp 逻辑，列表 H 自适应收缩。实现后用小终端冒烟。
- [对齐偏差] D2 的"值文本与 label 同列"在实现时若对错目标（条带左缘 vs label 列）会造成视觉次态 → 实现时以固定常量列渲染并冒烟核对；spec 只约束"同列"语义。
- [零候选渲染与 B 的重叠] B 的 draw 在零候选下仍渲染全结构；本 change 折叠后若 B 后续改动 draw 空态分支可能冲突 → 以 spec「零候选空态渲染」为行为准绳，折叠逻辑集中在一个明确分支内。
- [小终端高度不足] 26 高弹窗在 <24 行 pane 中可能贴边 → `render_modal_shell` 已对 `w/h` 做 `min(area…)` 钳制、`draw` 有高度 guard（现状 `inner.height < 14` return）；保持 clamp 逻辑，列表 H 自适应收缩。实现后用小终端冒烟。

## Migration Plan

- 纯渲染 change，无配置/数据迁移。顺序：B（source-jj-only）合并 → A 实现 → README 措辞（若有）→ spec 归档同步。
- 回滚：整体 revert A 提交；wizard 回到 pre-A 版式，无残留状态。

## Open Questions

- O1: D2 对齐目标的精确定义（值条文本列 = 列表 label 列 vs = marker 槽后列）——以"可见文本与列表项 label 同列"为验收语义，具体列偏移实现时定标并冒烟。
- ~~O2: 零候选时 tab/Enter 的确切行为~~ 已由 B 在 run 循环实现（`choices.is_empty()` 时 Tab/Enter 短路，main.rs:1386/1417），A 不再处理；draw 折叠后冒烟确认焦点不落隐藏 section 即可。
