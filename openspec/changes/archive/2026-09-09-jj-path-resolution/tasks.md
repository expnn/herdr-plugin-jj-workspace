# Tasks: jj-path-resolution

## 1. 前置

- [x] 1.1 确认 config-foundation 已实施：`load_config()` 与 `JjConfig` 存在且测试通过；本 change 在其 `JjConfig` 上增加 `command: JjCommandValue` 字段（untyped enum：`Name(String)` / `Path(String)` / `Argv(Vec<String>)`），默认 `Name("jj")`
- [x] 1.2 config-foundation 的 spec/tasks 中 `jj.command` 占位注释同步更新为"已定义"（README 示例同）

## 2. 解析层

- [x] 2.1 实现 `resolve_jj_command(value, path_dirs) -> Result<ResolvedJj, ResolveError>`：裸名按目录顺序 PATH 查找（存在 + 普通文件 + 可执行位，`PermissionsExt`）；绝对路径原样 + 校验；`~` 展开复用 config-foundation 的展开实现；含 `/` 的相对路径返回专用错误；argv 形态仅解析首元素
- [x] 2.2 构造错误信息：配置值原文 + 已搜索目录列表 + `which jj` 修复建议；argv 形态注明 `argv[0]`
- [x] 2.3 单元测试（注入临时目录列表，不碰进程 env）：裸名命中/未命中、不可执行条目跳过、绝对路径三种校验失败、相对路径拒绝、`~` 展开、argv 首元素解析、默认值 `"jj"`

## 3. 接入调用点

- [x] 3.1 `cmd_wizard`：配置加载后立即解析，失败在 TUI 渲染错误后退出；解析结果贯穿全部同步 `jj` 调用
- [x] 3.2 `cmd_remove`：同上（入口解析 + 失败非零退出）
- [x] 3.3 `jj_setup_command` / `right_pane_setup_command`（`src/main.rs:388/412`）：改为接受解析结果参数，命令串中全部裸 `jj` 替换为烘焙的绝对路径；argv 形态首元素同路径替换、其余元素以 sh 转义拼接
- [x] 3.4 更新/新增相关单测：`right_pane_materializes_then_bookmarks_then_updates`、`right_pane_setup_command_parses_under_posix_sh`、`right_pane_setup_command_parses_under_fish_when_available` 断言命令串不含裸 `jj`
- [x] 3.5 `grep -n '"jj"' src/main.rs` 复查：除解析层与测试 fixture 外无未烘焙的裸 `jj` 调用残留

## 4. 文档与验证

- [x] 4.1 README Configuration 章节：补 `jj.command`（string/argv 两种形态示例、解析规则简表、相对路径不支持）
- [x] 4.2 README 排障节：`herdr --remote` 自举 PATH 精简场景的症状与修复（`which jj` → 写入 `jj.command`）；说明解析唯一性（pane shell PATH 不参与）
- [x] 4.3 `cargo test` 全绿
- [x] 4.4 `cargo build --release` 后手工验收：正常 PATH 下默认行为不变；人为清空 PATH（如 `env -i` 启动 herdr server 模拟自举）时未配置 → fail-fast 错误含目录列表与修复建议；配置绝对路径后全流程（含右侧 pane 物化/bookmark/fetch/rebase）使用绝对路径成功
