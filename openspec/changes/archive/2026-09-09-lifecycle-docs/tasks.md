# Tasks: lifecycle-docs

## 1. Part B：remove 脏 @ 拒绝

- [x] 1.1 `cmd_remove` 在 forget 之前检测：`jj diff --summary -r @`（cwd = canon，用已解析的 `jj.executable` + extra_args）；stdout 非空或调用失败 → `die` 拒绝（消息三要素：检测到未提交修改 / 已提交内容与书签安全 / 先 `jj commit` 或 `jj restore` 后重试）
- [x] 1.2 单元测试（假 jj fixture，cwd 用真实临时目录）：脏 @ → die 且消息含出路与"bookmarks are safe"；干净 @（空输出）→ 流程继续；`jj diff` 失败 → fail-closed 拒绝
- [x] 1.3 手工验收（需真实 herdr 运行时）：仓库内制造未提交修改按 remove → 拒绝、目录仍在；干净副本 remove → 正常删除

## 2. Part A：README 生命周期章节

- [x] 2.1 新增 "How jj workspaces relate" 章节：commit/bookmark 层跨工作副本即时共享（`.jj/repo` 文件指针）、工作副本文件物化彼此隔离、回主线为手动 jj 工作流（插件不做自动同步）、remove 语义（forget + rm -rf、主工作区永不删除、脏 @ 拒绝）、交叉引用 Choosing the base revision
- [x] 2.2 Troubleshooting 交叉补充：remove 被拒绝时的处理路径一行

## 3. 验证

- [x] 3.1 `cargo test` 全绿；`cargo build --release` 零警告
- [x] 3.2 openspec validate 通过
