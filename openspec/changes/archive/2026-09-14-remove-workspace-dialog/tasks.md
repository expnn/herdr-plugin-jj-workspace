# Tasks: remove-workspace-dialog

## 1. Manifest 与入口

- [x] 1.1 `herdr-plugin.toml`：新增 `[[panes]] id = "remove-wizard"`（title "Remove jj workspace"、placement overlay、command `["./target/release/jj-workspace", "remove-wizard"]`）
- [x] 1.2 `src/main.rs`：`cmd_remove` 拆分为预检（config/`jj.command` 预解析 + 目标上溯 + 非 jj/不安全路径 toast 守卫）+ `herdr plugin pane open --entrypoint remove-wizard --focus`（对齐 `cmd_open` 形态）；删除旧的 headless 执行路径与 `herdr tab close` 调用
- [x] 1.3 `src/main.rs`：新增 `remove-wizard` 子命令与 `cmd_remove_wizard` 入口（自身 context 重新解析目标；失败渲染 fail-fast modal）

## 2. 目标解析与数据源

- [x] 2.1 目标解析：`focused_pane_cwd`（fallback `workspace_cwd`）逐级上溯最近含 `.jj` 的祖先；主 workspace / 非 jj / 路径不存在 / 不安全路径判定与结果分类（view mode 选择）
- [x] 2.2 picker 枚举：执行 `jj -R <main_root> --ignore-working-copy workspace list -T 'name ++ "\t" ++ root ++ "\n"'` 并解析 `name`/绝对 `root`；空 root 记 stale；按 canonical path 排除主 workspace；fake jj 单测
- [x] 2.3 pane 扫描与过滤：解析 `herdr pane list`（字段缺失容错）；候选 = `cwd` ∪ `foreground_cwd` 位于目标目录内（canonicalize + 路径边界比较）；按 `workspace_id`/`tab_id` 分组并从 `herdr workspace list` / `tab list` 取展示 label；自 pane 排除规则（`cwd == plugin_root` 且 `label` 属于本插件 pane 标题）；fake herdr 单测
- [x] 2.4 checks 数据源：clean 检查（`jj diff --summary -r @`，输出行数）与 opencode inspect 结果接入审阅数据模型

## 3. 对话框 TUI

- [x] 3.1 状态机与选择模型：Picker / Review / Status / commit 子态；session 勾选、pane 树勾选（组头级联、`a` 全局全选/全不选、默认全选、计数、0 选中警示、`(triggered here)` 标记）实现为纯函数并单测
- [x] 3.2 Picker 视图：名称 + 绝对路径 + `(missing on disk)` 渲染；空态；`↑↓ / ↵ / esc` 键位
- [x] 3.3 Review 视图：Workspace / Plan（4 任务 + session 列表 + pane 层级树）/ Checks 渲染；滚动与自适应高度（hint 与按钮固定）；hint 随模式切换；错误/阻断行
- [x] 3.4 阻断与修复交互：✗ 检查时 `↵` 不执行并显示阻断摘要；脏时 `c` 进入单行 message 输入，`↵` 以目标目录为 cwd 执行 `jj commit -m <message>` 后刷新检查；`jj restore` 仅展示（无执行路径）
- [x] 3.5 Status 视图：按序任务状态（✓/◌/·/✗）、成功自动退出、失败停留 + `error.log` 指针 + `↵ close`
- [x] 3.6 验收反馈渲染修订：点号序号 `1.`–`4.`；宽度 96 / 高度 clamp `[31, min(终端高-4, 45)]`；全路径 `$HOME`→`~` 缩写；opencode 就绪行两行式（dim 缩进 `migrate to <path>`）
- [x] 3.7 验收反馈滚动与计数：对话框启用 mouse reporting（herdr 侧 `MouseReport`），滚轮滚动内容且不改动光标（独立 `scroll` 偏移）；内容溢出显示滚动条（review + picker）；「N of M selected」计数行实时渲染于步骤 1/4 下

## 4. opencode 迁移扩展

- [x] 4.1 `src/opencode_migration.rs`：新增 `inspect` 只读预览（复用 find / discover / `probe_schema` / 范围谓词；返回行 `{id, title, directory, time_updated}` + 主仓库根 / 拒绝原因 / 跳过；不写库）
- [x] 4.2 迁移子集参数：仅更新勾选 id；未勾选行不动；全未勾选按跳过；预览与执行间漂移按实际更新数上报
- [x] 4.3 迁移单测：新增 inspect（行、拒绝、跳过、不写库）与子集迁移用例；现有 11 个用例语义保持
- [x] 4.4 DB 定位回退（验收回归修复）：`find_opencode` 追加常见安装目录（~/.opencode/bin、~/.local/bin、/usr/local/bin）；CLI 不可用/输出无效时回退 `$XDG_DATA_HOME/opencode/opencode.db`（默认 `~/.local/share/opencode/opencode.db`）；预览与执行共用发现链；跳过原因指明已尝试位置；配套单测

## 5. 执行管线

- [x] 5.1 `↵` 时复检 clean（失败回审阅、零步骤执行）
- [x] 5.2 依序执行：迁移（勾选子集）→ `jj workspace forget` → 删除目录（缺失跳过）→ 关闭勾选 pane（逐项 best-effort、跳过自 pane、单项失败仅警告）
- [x] 5.3 阶段门控：破坏性步骤失败即停（迁移失败不 forget；forget 失败不删目录；删除失败不关 pane），Status 说明已完成/未执行 + `error.log` 写入

## 6. 文档

- [x] 6.1 README "Removing a workspace" 章节重写（对话框流程、picker、session/pane 勾选、修复指引、按 pane 关闭语义）
- [x] 6.2 README Troubleshooting：`refusing to remove … uncommitted changes` 条目更新为对话框阻断语义与 `c` commit 出路
- [x] 6.3 `openspec validate --change remove-workspace-dialog --strict` 通过

## 7. 验证

- [x] 7.1 `cargo test` 全绿（新增：目标解析、picker 解析、pane 过滤与自排除、选择模型、inspect/子集、管线门控）
- [x] 7.2 `cargo build --release` 成功
- [x] 7.3 真实冒烟（`/tmp/opencode/herdr-0.8.2` + 本地 link 插件 + `/tmp/opencode` 下临时 jj 仓，不触碰既有 workspace）：① 子目录触发解析到根；② 主 workspace picker（含 stale 项与仅 forget 路径）；③ 脏工作副本阻断 + `c` commit 后放行；④ session 勾选子集迁移；⑤ 多 pane tab 部分勾选（tab 存活）与全勾选（级联关闭）；⑥ 0 pane 选中警示；⑦ 成功 Status 与自动退出、失败 Status 与 error.log
- [ ] 7.4 兼容复验（非完成判据，server 升级后补）：0.9.0 下 `pane list` 字段与 overlay 行为一致性
