# Tasks: config-foundation

## 1. 配置解析层

- [x] 1.1 在 `Cargo.toml` 添加 `toml` 依赖（确认 `serde`/`serde_derive` 版本兼容）
- [x] 1.2 定义 `Config` 结构体：`JjConfig { base_rev, workspace_root }` + `AgentConfig { command, bootstrap_paths }`，带 `#[serde(deny_unknown_fields)]`；内置默认值与现状逐字节一致（`trunk()` / `~/.herdr/workspaces` / `codex` / 现 `CODEX_BOOTSTRAP_PATHS` 列表）
- [x] 1.3 实现 `load_config() -> Result<Config, ConfigError>`：从 `HERDR_PLUGIN_CONFIG_DIR/config.toml` 读取；文件缺失 = 全默认；语法/类型/未知键错误返回携带键名与原因的 `ConfigError`；空字符串值返回专用错误
- [x] 1.4 实现 `jj.workspace_root` 的 `~` 展开；`agent.bootstrap_paths` 保持仓库相对、不展开
- [x] 1.5 单元测试覆盖：文件缺失、正常解析、语法错误、类型错误、未知键、空字符串、`~` 展开（用临时目录写 fixture）

## 2. 接入现有调用点（行为不变迁移）

- [x] 2.1 `cmd_wizard`：入口调用 `load_config()`，失败时在 TUI 渲染错误信息后退出；成功后用 `Config` 替代所有 `config_value("JJ_*")` 调用
- [x] 2.2 `resolve_start_command()`：改为从 `Config.agent.command` 取值；空串语义随 1.3 移除（空值在解析层已被拒绝），更新对应单元测试
- [x] 2.3 `open_tab_layout` / 右侧 pane setup 命令生成：bootstrap 文件列表改用 `Config.agent.bootstrap_paths`（默认值 = 原硬编码，命令形态不变）
- [x] 2.4 `cmd_remove` 及其余 `herdr`/`jj` 调用点：清理所有 `config_value` 引用，确认无 `JJ_*` 环境变量读取残留（`grep -n "JJ_BASE_REV\|JJ_START_COMMAND\|JJ_WORKSPACE_ROOT\|CODEX_BOOTSTRAP_PATHS" src/` 应为零命中）
- [x] 2.5 删除 `config_value()` 与 `.env` 解析逻辑；删除 `.env.example` 文件

## 3. 文档

- [x] 3.1 重写 README「Configuration」章节：config.toml 路径（`herdr plugin config-dir nathanflurry.jj-workspace` 获取）、完整带注释示例（含 `jj.command` 与 `agent.auto_trust` 占位注释并标注"由后续 change 定义"）、从 `.env` 迁移的键名对照表
- [x] 3.2 README 写明 `agent.command` 的执行模型：经 `herdr pane run` 敲入 pane 交互式 shell，支持别名/函数/shell 语法及机制前提
- [x] 3.3 README 写明 `agent.bootstrap_paths` 的两个已知行为：不存在的路径由 jj 静默跳过、模式常驻 sparse list（仓库将来出现同名文件会自动物化）

## 4. 验证

- [x] 4.1 `cargo test` 全绿（含新增解析层单测）
- [x] 4.2 `cargo build --release` 后手工验收：无 config.toml 时创建工作区行为与迁移前一致（默认值路径）
- [x] 4.3 手工验收 fail-fast：写入语法错误 / 未知键 / 空字符串的 config.toml，分别确认 wizard 报错与 action 非零退出
- [x] 4.4 手工验收覆盖路径：`jj.workspace_root = "~/..."`、`agent.command` 用 shell 别名启动 agent、`bootstrap_paths` 含不存在的路径
- [x] 4.5 验证 `.env` 与 `JJ_*` 环境变量彻底失效（遗留 .env 存在时不被读取）
