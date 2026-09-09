# Delta Spec: plugin-config

## ADDED Requirements

### Requirement: jj.command 键与取值形态
配置 SHALL 支持 `[jj] command` 键，取值为字符串（可执行文件路径或裸名，不含参数）或字符串数组（完整 argv，其中仅 argv[0] 为可执行文件）。默认值 SHALL 为 `"jj"`。

#### Scenario: 默认值
- **WHEN** config.toml 中未定义 `jj.command`
- **THEN** 插件按裸名 `"jj"` 在 PATH 中查找并使用查得的绝对路径执行所有 jj 调用

#### Scenario: argv 形式仅解析首元素
- **WHEN** `jj.command = ["/opt/jj/bin/jj", "--at-op", "@-"]`
- **THEN** 插件校验 `argv[0]`（`/opt/jj/bin/jj`）为可执行文件并原样使用，其余元素在调用时原样拼接

### Requirement: jj.command 路径解析规则
对 `jj.command`（或 argv 形式的 argv[0]）：以 `/` 开头的绝对路径 SHALL 原样使用；不含 `/` 的裸名 SHALL 在插件进程的 `PATH` 环境变量中按目录顺序查找（条目存在 + 普通文件 + 可执行位，目录缺失或空段静默跳过）；`~` 开头的值 SHALL 先做 `~` 展开再按上述规则处理。

#### Scenario: 裸名 PATH 查找
- **WHEN** `jj.command = "jj"`，且 PATH 中 `/opt/tools/bin` 目录下存在可执行的 `jj`
- **THEN** 插件解析出 `/opt/tools/bin/jj` 并以该绝对路径执行所有 jj 调用

#### Scenario: PATH 中跳过不可执行的同名条目
- **WHEN** PATH 前部某目录存在同名 `jj` 但无可执行位，其后目录存在可执行的 `jj`
- **THEN** 插件跳过不可执行条目，解析到其后目录中的可执行文件

#### Scenario: ~ 展开后按规则处理
- **WHEN** `jj.command = "~/.local/bin/jj"` 且该文件存在且可执行
- **THEN** 插件展开为绝对路径并原样使用

### Requirement: 相对路径拒绝
包含 `/` 但不以 `/` 开头的 `jj.command` 值（如 `./bin/jj`、`bin/jj`）MUST 被拒绝（fail-fast），错误信息 SHALL 说明只接受裸名（无 `/`，按 PATH 查找）或绝对路径。

#### Scenario: 相对路径报错
- **WHEN** `jj.command = "./bin/jj"`
- **THEN** 插件拒绝运行（wizard 界面显示错误 / action 非零退出），错误信息指出相对路径不被接受

### Requirement: 可执行检测失败时 fail-fast 并给出修复指引
解析或校验失败（裸名在 PATH 中找不到、绝对路径不存在/非普通文件/不可执行）时，插件 MUST 以与 config-foundation 相同的 fail-fast 通道失败（wizard TUI 渲染错误；headless action 非零退出）。错误信息 MUST 包含：配置值原文、（裸名时）已搜索的全部 PATH 目录列表、修复建议（"在你的 shell 中运行 `which jj`，将结果写入 config.toml 的 `jj.command`"）。argv 形式的错误信息 SHALL 注明错误来自 `argv[0]`。

#### Scenario: 裸名找不到时的错误内容
- **WHEN** `jj.command = "jj"` 且 PATH 所有目录中均无可执行 `jj`
- **THEN** wizard 打开时界面显示错误并列出已搜索的目录；`new`/`remove` action 以非零码退出，toast 显示失败；错误文本含 `which jj` 修复建议

#### Scenario: 绝对路径不可执行
- **WHEN** `jj.command = "/opt/jj/jj"` 且该文件存在但无可执行位
- **THEN** 插件失败，错误信息指明该路径不可执行

### Requirement: 解析唯一性——一次解析处处使用
插件 SHALL 在每次调用的入口（wizard / remove）对 `jj.command` 解析恰好一次并在进程内贯穿使用：进程内同步 `jj` 调用直接使用解析结果；右侧 pane setup 命令串中的全部 `jj` 调用 SHALL 以解析出的绝对路径烘焙（sh 转义语义不变）。插件 MUST NOT 在右侧 pane 命令串中保留依赖 pane shell PATH 的裸 `jj`。

#### Scenario: 右侧 pane 命令使用绝对路径
- **WHEN** `jj.command` 解析为 `/opt/tools/bin/jj`，用户通过 wizard 创建工作区
- **THEN** 右侧 pane 收到的 setup 命令串中，sparse 物化 / bookmark 创建 / fetch / rebase 均以 `/opt/tools/bin/jj` 调用，不含裸 `jj`

#### Scenario: 插件进程与 pane shell 的 PATH 差异不影响结果
- **WHEN** server PATH 中无 jj（已通过 `jj.command` 显式配置），右侧 pane 用户 shell 的 PATH 中有另一个 `jj`
- **THEN** 所有 jj 调用（含右侧 pane）均使用 `jj.command` 解析出的绝对路径，pane shell 的 PATH 不参与解析
