## Context

现状见 proposal.md「Why」。与本设计相关的既有约束（均以当前主 spec 为准）：

- 阻断判定与文案集中在纯函数 `review_blocking_reason(&ReviewData)`（`src/main.rs:3277`），`data.clean: Option<Result<Vec<String>, String>>` 同时编码「路径未知(None)」「干净(Some(Ok(空)))」「脏(Some(Ok(非空)))」「检测失败(Some(Err))」四种状态。
- 审阅行由纯函数 `build_review_rows(&ReviewData)` 单一产出，`ReviewModel` 在其上做光标/勾选/滚动；`model.replace_rows(...)` 在复检与 commit 后刷新。
- 授权前的 TOCTOU 复检只发生在 review 的 `KeyCode::Enter` 分支（`src/main.rs:4398-4418`）；执行管线 `run_remove_pipeline` 不再复检。
- `DialogMode` 已有 `Review` / `Commit` 两态，共用 `draw_review_dialog` 与单行编辑子态机制。
- 脏 `@` 在被 `jj workspace forget` 后以匿名 commit 留在共享存储（spike 结论，见项目 memory）。

## Goals / Non-Goals

**Goals:**

- 增加一条**显式、可撤销、需二次确认**的强制删除授权路径，只放开「成功的脏检测」这一个门禁。
- 复用既有审计/展示机制（`clean` 检查行、`ReviewModel` 勾选、`DialogMode`、Status 视图），不引入新的 jj 命令或新的执行阶段。

**Non-Goals:**

- 不引入 `jj restore` 或任何新的破坏性 jj 调用；不改变脏 `@` 的存储去向。
- 不改变其余阻断项（opencode DB 不可读、迁移 fail-closed）与 `jj diff` 检测失败的 fail-closed 语义。
- 不改变干净路径的交互（无强制行、无警告页）。

## Decisions

### 决策 1：强制开关做成 Checks 区的可勾选行，而非快捷键

在 `build_review_rows` 的 Checks 段、脏检测行的出路指引之后，追加一个专用可选行（独立 `ReviewRow` kind + 固定 id，如 `FORCE_ROW_ID`）。仅当 `clean == Some(Ok(changes))` 且 `!changes.is_empty()` 时产出。

理由：用户要求「勾选」；复用 `ReviewModel` 已有的叶子行 toggle / `space` 处理 / 光标跟随，无需新增键位与 hint 改动；干净或检测失败时该行自然不产出。

备选（未采纳）：`f` 快捷键切换。与「勾选」表述不符，且需新增 hint 键位、独立于行选择模型。

### 决策 2：`review_blocking_reason` 接受 force 参数，只在「成功检测到脏」分支放行

改为 `review_blocking_reason(data, force: bool)`：`Some(Err(_))` 分支始终返回阻断（force 无效）；`Some(Ok(非空))` 分支在 `force == true` 时返回 `None`，否则维持原阻断摘要。`SessionPreview::Refused` 分支不受 force 影响。

理由：把唯一的新语义收在最窄的分支里，明确划定「force 只绕过成功的脏检测」，避免动到检测失败的 fail-closed。

### 决策 3：新增 `DialogMode::ForceConfirm` 警告页，`↵` 两步授权

review 的 `↵`：先执行既有复检并 `replace_rows`；随后用 `(data, force)` 调 `review_blocking_reason`：

- 返回 `Some(reason)` → 显示错误（含检测失败与未开启 force 的脏）。
- 返回 `None` 且 `data` 仍为成功脏 + force 开启 → 进入 `DialogMode::ForceConfirm`（不授权）。
- 否则 → 直接授权。

`ForceConfirm` 页复用 `draw_review_dialog` 的骨架渲染警告块（目标路径 + 待放弃改动逐行/截断 + 后果说明），hint 为 `↵ confirm force remove · esc back`。`↵` 授权；`esc` 回到 `Review`（保留行状态，但授权必须再经 `↵` 的复检路径）。

理由：与 `Commit` 子态同构，改动集中；警告内容复用 `clean` 的改动列表，不重复取数。备选（未采纳）：在警告页再次复检——两次复检窗口极短，收益低且会让「展示的改动数」与「授权时的实际状态」产生第二次漂移；现有设计已由首次复检收敛（与既有 spec 的「确认时复检」一致）。

### 决策 4：force 为瞬态，刷新即重置

force 存放在对话框事件循环的局部状态（或 `ReviewModel` 上单独字段），在每次调用 `replace_rows` 的刷新路径（授权前复检、`c` commit 成功后）一并清为未开启。

理由：防止一次勾选后经刷新静默持续放行；强制必须是「当下这次」的显式决定。

### 决策 5：`RemovePlan` 用 `forced: Option<usize>` 记录

`to_plan` 将 force 开启时的脏改动数写入 `forced = Some(n)`，否则 `None`。`run_remove_pipeline` 相应在删除目录步骤的 Status 文案里输出 `forced: abandoned N uncommitted change(s)`。

理由：单字段同时表达「是否强制」与「放弃了多少」，供 Status 审计；无需管线内重新取数（授权到执行之间没有 jj 命令改动工作副本）。

## Risks / Trade-offs

- [用户在警告页停留期间别处新增改动会被一并放弃] → 警告页展示的改动数来自复检后的最新状态；用户已通过两步确认接受「放弃当前全部未提交改动」的语义。
- [refresh 未重置 force 造成静默绕过] → 决策 4 指定在所有 `replace_rows` 路径统一清位；以纯函数测试覆盖「刷新后 force 归零」。
- [新增可选行改变行数，可能影响既有光标/滚动不变量] → 强制行只在脏时出现，且经既有 `replace_rows` + `follow_cursor` 走同一更新路径；补充行产出条件与 toggle 的纯函数测试。
- [Status 文案携带的数字与最终放弃内容不一致] → 授权后管线不再产生工作副本改动，数字稳定；若用户在执行前手动改动，属对话框外操作，超出本设计的承诺边界。

## Migration Plan

无数据迁移、无 config 变更、无发布兼容影响；纯交互增强。回滚即移除新分支与行。

## Open Questions

_无_
