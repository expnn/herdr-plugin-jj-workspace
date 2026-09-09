# Design: lifecycle-docs

决策 D1-D3。实验依据：bmspike 实验仓（jj 0.45.1，结论存于项目记忆 #148）。

## D1 remove 检测收窄为"脏 @"，不做书签/未推送警告

实验事实：

| 对象 | `jj workspace forget` 后 |
|---|---|
| 脏 @（未提交物化修改） | **静默成功（exit 0）**；脏 commit 以匿名形式存活于共享存储；物化文件被插件 `rm -rf` 后消失 → 用户视角 = 工作丢失 |
| 已提交 commit + 书签 | 完整存活（书签在共享存储，跨工作副本可见） |

推论：唯一需要拦截的是脏 @。为书签/未推送加警告是噪音（它们不会因 remove 丢失），反而稀释真正重要的拒绝信号。

检测命令：`jj diff --summary -r @`（cwd = 待删工作副本）——干净时输出为空，脏时逐行列出。比解析 `jj status` 的人类输出更稳。

## D2 拒绝（die）而非警告继续

`remove` 是 headless action（herdr 面板按钮触发），无 TTY、无带参重试通道。可选行为只有两种：拒绝执行（当前修改原样保留，用户在终端里 `jj commit` / `jj restore` 后重按按钮）或警告后照删（明知有丢失风险还放行）。后者与插件"删整个目录"操作的分量不匹配——拒绝是诚实的选择。

拒绝消息三要素：发生了什么（uncommitted changes detected）、哪些不受影响（already-committed work and bookmarks are safe in the repo store）、出路（commit or `jj restore` first, then run remove again）。

## D3 无新配置键

没有 config 键能改善这个交互：headless action 读不到运行时用户意图；"跳过检查"的唯一合理入口是把仓库先变干净（这总是可行）。保持零配置面。

## Part A：README 章节大纲（"How jj workspaces relate"）

- 一个仓库、多个工作副本：副工作区 `.jj/repo` 是文件指针，共享同一存储
- commit/bookmark 即时全局可见（change ID 一致）；磁盘文件物化彼此隔离——这是特性
- 回主线 = 手动 jj 工作流（commit → rebase/push 可在任一工作副本做）；插件不做自动同步
- remove 语义：`workspace forget` + 目录删除；主工作区永不删除（代码已保护）；**存在未提交修改时拒绝执行**
- 交叉引用：创建时 base_rev 的解析与 rebase 目标（链接到 Choosing the base revision 章节）

## 测试策略

- 脏 @ 拒绝：假 jj fixture（`diff --summary -r @` 输出非空）→ 断言 die 消息、且 forget/`rm -rf`/tab close 未执行
- 干净 @：空输出 → 照常全流程
- `jj diff` 失败（非仓库等）：视为拒绝（fail-closed）——宁可误拒不误删
- 手工验收：真实仓库内制造脏工作副本按 remove → 拒绝消息出现、目录仍在；干净副本 remove → 正常删除
