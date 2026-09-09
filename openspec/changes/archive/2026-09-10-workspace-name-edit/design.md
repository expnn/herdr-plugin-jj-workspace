# workspace-name-edit Design

## Context

wizard 的 name 字段当前用单个 `replace_on_type: bool` 承载"未编辑"标记（main.rs:1356）。`replace_on_type == true` 时，首字符输入（Char 分支，main.rs:1482-1485）与 Backspace（main.rs:1468-1476）都会先 `name.clear()` 清掉**整个**自动生成的默认值 `workspace/<slug>`。

讨论定案（proposal）：新建 workspace 保留 `workspace/` 命名空间（workspace 名/书签名/目录 slug 三处同源）。当前整串替换交互与该结论相悖——用户只想改随机生成的 `<slug>`，却被迫重输前缀。

## Goals / Non-Goals

**Goals:**
- name 字段编辑细化为组件级：首字符保留前缀、Backspace 先删 slug 再删前缀
- 保证精细编辑安全：一旦用户开始自由编辑，Backspace 恒为逐字符

**Non-Goals:**
- 不改 base 字段行为（`trunk()` 无组件结构）
- 不改 `generated_name` / `branch_to_path_slug` / 前缀字面量本身
- 不做前缀的自定义/配置化
- 不改提交校验（`valid_branch`）

## Decisions

### D1: 三态编辑状态机替代 `replace_on_type: bool`

```rust
enum NameEditState {
    Fresh,     // name = 自动生成的完整默认值 "workspace/<slug>"（未编辑）
    Prefixed,  // name = "workspace/"（slug 已清，前缀保留）
    Free,      // 自由编辑：逐字符
}
```

转移（name 字段聚焦时）：

| 状态 | Char 输入 | Backspace |
|---|---|---|
| Fresh | 裁掉 slug、保留前缀后追加 → Free | 删 slug → Prefixed |
| Prefixed | 追加字符 → Free | 清空 name → Free |
| Free | 追加字符 | `pop()` 一个字符 |

**备选**：双 bool（`slug_cleared` + `replace_on_type`）——语义不如三态枚举清晰，状态组合易错，否决。

### D2: 组件级操作只作用于未编辑锚点（安全阀）

组件级删除（整删 slug、整删前缀）**只**在 Fresh/Prefixed 锚点态生效；一旦发生任何字符输入进入 Free，Backspace 恒逐字符。

理由：`workspace/fix-ap1` 改错一位时，Backspace 若整删 slug 是破坏性的。锚点态删除安全的前提是"被删内容是自动生成的、无用户手输信息"。此规则同时保证**组件级删除永不触及用户手输内容**——因此无需硬编码匹配 `workspace/` 前缀：用户自输 `feature/foo` 天然是 Free 态，恒逐字符。

### D3: 前缀从 name 自身推导，不硬编码

Fresh 态 Backspace/Char 需要"前缀"：`name.rsplit_once('/')` 取前半 + `'/'`，即 `workspace/`。不引入 `const PREFIX` 常量，避免与 `generated_name` 的格式重复维护。

### D4: Prefixed 态显示保留尾斜杠

`workspace/` 原样显示，视觉上"等你输入 slug"。不新增灰显/高亮样式（属可选增强，见 Non-Goals 之外不做）。

### D5: base 字段保持现状

`base_replace_on_type` 语义不变。两个字段独立演化，互不干扰。

## Risks / Trade-offs

- **Free 态删除前缀需逐字符** → 接受：换取"改错一字不灾难"的安全；两次 Backspace 路径已覆盖"主动清除前缀"的高频意图。
- **锚点态下用户若已部分感知为"逐字符"会惊讶**（首次 Backspace 删整个 slug）→ 缓解：锚点态仅存在于未输入任何字符前，属于"清空默认值"的直觉语义；错误行机制不变。
- **Fresh 态 name 无 `/` 的防御**（理论上不发生）→ `rsplit_once` 为 None 时退化为清空，行为等价当前实现。
- **显示 `workspace/` 尾斜杠的观感** → 与输入状态自洽（斜杠即"分隔待填"），保持普通样式。

## Migration Plan

纯交互行为变更，无配置/数据迁移。存量已创建 workspace 名不受影响。

## Open Questions

无（交互语义已与用户逐条定案：Backspace 两级、首字符保留前缀、自由态逐字符）。