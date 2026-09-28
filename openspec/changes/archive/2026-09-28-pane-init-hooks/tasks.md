# Tasks: pane-init-hooks

## 1. 配置解析与校验（`[init]` 节）

- [x] 1.1 `src/main.rs` 新增 `InitConfig`（字段 `default`/`left`/`right`，类型 `Option<String>`，`#[serde(deny_unknown_fields, default)]` + `Default`/`PartialEq` 派生），并挂到 `Config`（新增 `init: InitConfig` 字段与 `Config::default()` 取值）；验证：`cargo test` 既有配置测试全绿（缺省 `[init]` 的配置照常解析）。
- [x] 1.2 `Config::validate` 对三键做显式空串拒绝（`ConfigError::Empty`，错误定位 `init.default` / `init.left` / `init.right`）；验证：新增 tempdir 配置测试——`[init] left = ""` 加载失败且错误文本含 `init.left`，`[init] left = "x"` 与三键缺席均加载成功。
- [x] 1.3 新增每 pane 解析纯函数（如 `resolved_init(init: &InitConfig, pane: InitPane) -> Option<&str>`）：显式键优先、缺席回退 `default`、两者皆缺席为 `None`；验证：单元测试覆盖左/右 × {显式、回退、全缺席} 六种组合，且断言"显式键不叠加 `default`"。

## 2. 命令组合与注入

- [x] 2.1 新增组合纯函数（如 `compose_pane_command(init: Option<&str>, main: &str) -> String`）：`None` → `main.to_string()`，`Some` → `format!("{init} && {main}")`；验证：单元测试断言 None 分支与输入逐字节相等、Some 分支精确等于 `<init> && <main>`（单空格）。
- [x] 2.2 令 `open_tab_layout` 可注入 herdr 路径：`herdr_bin()` 上移到 `cmd_wizard` 调用处并以参数传入（生产行为不变，调用点仅多一次变量传递）；验证：既有测试全绿，`open_tab_layout` 内不再读 env。
- [x] 2.3 `open_tab_layout` 用解析结果组合两侧注入：左 = `compose(左解析, resolve_start_command(&config.agent))`，右 = `compose(右解析, setup_script_command(...))`；验证：由 2.4/2.5 的测试覆盖。
- [x] 2.4 假 herdr 集成测试（经 2.2 的参数注入假 herdr：应答 `tab create` / `pane split` 的 JSON envelope，并把每次 `pane run` 参数写入日志）：断言四种场景下两条 `pane run` 的完整注入文本——(a) 无 `[init]`：逐字节等于改造前；(b) 仅 `default`：两侧各带同一前缀；(c) `default` + `left` 覆盖：左侧自有前缀、右侧 `default` 前缀；(d) 仅 `left`：仅左侧带前缀、右侧逐字节不变；验证：`cargo test` 中该集成测试通过（假 herdr 必须应答成功——`open_tab_layout` 失败路径是 fail-fast 退出）。
- [x] 2.5 右 pane 回归断言：带 init 前缀时 `setup_script_command` 的输出部分与无前缀时逐字节相同（复用/扩展 `setup_script_command_is_a_single_plain_call` 的断言方式）；验证：该测试通过。

## 3. 文档

- [x] 3.1 README.md Configuration 节新增 `[init]` 子节：三键示例、解析链（显式键覆盖 `default`，覆盖非叠加）、`&&` 失败语义（可自写 `|| true` 软化）、pre-only 边界（钩子执行时工作副本只有引导文件）、shell 可移植性（`. /path`/`source` 须匹配 pane 的 `$SHELL`；跨 shell 用 `sh /path`）、`right = "true"` 显式关闭变通、失败可见性（左：agent 不启动 → finish-tab 超时提示；右：checkout 不跑、错误留在右 pane）；验证：逐条对照 `openspec/changes/pane-init-hooks/specs/plugin-config/spec.md` 的场景核对无遗漏。
- [x] 3.2 README Quickstart 段落补一句：新 tab 两个 pane 可分别配置主命令前的初始化命令（指向 Configuration 节）；验证：从 Quickstart 文本可循迹到 `[init]` 说明。

## 4. 验证

- [x] 4.1 `cargo test` 全绿（含本 change 新增的单测与集成测试）
- [x] 4.2 `cargo build --release` 成功
- [x] 4.3 真实 herdr 端到端冒烟（在一次性测试仓库 + 临时 `workspace_root` 中进行）：写入 `[init] default = "sh -c 'echo default >> /tmp/pane-init.log'"` 并经 wizard 创建 workspace，确认两个 pane 各写入一行、checkout 与 agent 启动正常；再以 `left = "false"` 覆盖，确认左 pane 不启动 agent 且 finish-tab 给出未检测到 agent 的提示；验证：日志内容/行数与 pane 观察结果符合预期。
- [x] 4.4 `openspec validate pane-init-hooks --strict` 通过
