## 1. 纯函数与数据模型

- [x] 1.1 `review_blocking_reason` 增加 `force: bool` 参数：`Some(Err)` 分支始终阻断；`Some(Ok(非空))` 在 `force` 时返回 `None`、否则维持原摘要；`SessionPreview::Refused` 不受影响。更新全部调用点并补充单测（脏+force 不阻断、脏+无 force 阻断、检测失败+force 仍阻断），`cargo test` 通过
- [x] 1.2 `build_review_rows` 在 Checks 段脏检测行的出路指引后产出强制删除行（独立行 kind + 固定 id），仅当 `clean == Some(Ok(changes))` 且非空时产出；补充单测覆盖成功脏/干净/检测失败/路径未知四种产出情况，`cargo test` 通过
- [x] 1.3 `RemovePlan` 增加 `forced: Option<usize>`，`to_plan` 在 force 开启时写入脏改动数、否则 `None`；补充单测断言两种取值，`cargo test` 通过

## 2. 审阅交互与警告页

- [x] 2.1 新增 `DialogMode::ForceConfirm`：review `↵` 分支在复检后按 `(data, force)` 判定——成功脏且 force 开启进入警告页，否则维持阻断或直接授权；`draw_review_dialog` 渲染警告块（目标路径 + 待放弃改动逐行与截断 + 后果说明）并输出警告页 hint `↵ confirm force remove · esc back`。手动打开对话框验证进入警告页的渲染与文案
- [x] 2.2 force 行支持 `space` 切换：在 `ReviewModel` 上增加 force 状态并接入既有 toggle 路径；在所有 `replace_rows` 刷新路径（授权前复检、`c` commit 成功后）将 force 重置为未开启。补充单测覆盖 toggle 与刷新重置，`cargo test` 通过
- [x] 2.3 警告页 `↵` 授权（携带 `forced` 的 plan）、`esc` 返回 review 且零变更；补充/复用纯函数或事件用例验证两个分支，`cargo test` 通过

## 3. Status 与执行

- [x] 3.1 `run_remove_pipeline` 的删除目录步骤在 `plan.forced` 为 `Some(n)` 时输出 `forced: abandoned N uncommitted change(s)` 的 Status 文案；复用或新增断言验证该文案，`cargo test` 通过

## 4. 验证

- [x] 4.1 `cargo test` 全绿且 `openspec validate force-remove-dirty-workspace --strict` 通过
- [x] 4.2 手动端到端：在带未提交改动的副 workspace 上走强制删除，确认全程无 `jj commit`/`jj restore`、目录被删除、脏 `@` 以匿名 commit 存活于共享存储（`jj log -r all()`）、Status 显示 forced 计数；制造 `jj diff` 检测失败，确认强制行不出现且删除被阻断
