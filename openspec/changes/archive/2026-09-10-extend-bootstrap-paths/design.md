# Design: extend-bootstrap-paths

决策 D1-D7。背景讨论见会话（Q1-Q5 全部关闭）；代码依据为 `src/main.rs` 现状（`AgentConfig` 的容器级 `#[serde(default)]` + `Default` 填 34 项；`load_config_from` 解析后 `validate`）。

## D1 语义：永远叠加（always-union）

生效清单 = 已解析的 `bootstrap_paths` ++ `extend_bootstrap_paths`。其中"已解析的 `bootstrap_paths`"沿用既有逻辑：文件显式值，若有；否则 34 项内置默认。

选择 always-union 而非"仅默认时生效"的理由：后者在"用户同时设置两者"时自相矛盾（"我完全接管" vs "我还要扩展"），只能报错或静默忽略一方——凭空引入新失败态或静默行为。而"完全接管 + 还要加"的真实诉求已被白名单模式（D4：`[]` + extend）覆盖，Q1 不需要分支。

被否方案：为区分"缺席键"与"显式值"引入 presence tracking（`Option<Vec<String>>`）。不需要——extend 直接拼在**已解析好**的 `bootstrap_paths` 后面，用户描述的行为是该语义的特例（extend 为空时逐字节等同今天）。

## D2 去重：保序去重

extend 条目若已在基线中，生效清单只保留首次出现位置。`jj sparse set --add` 重复传同一 pattern 虽幂等无害，但去重让 sparse 列表干净、消除"我是不是配重了"的困惑；且当未来默认新增用户 extend 中已有的项时，合并保持干净（delta 声明 vs 快照复制的核心优势：默认演进自动继承）。

## D3 不要减法机制

默认清单是纯超集且无害（不存在的路径 jj 静默跳过；存在的全是 KB 级指令文件），减法没有真实用例：省不下可感知的时间（全量物化数秒后即到），也无隐私问题（全是自己的 repo）。若未来真有排除需求，再加 `exclude_bootstrap_paths`（gitignore `!` 式语义为现成类比）——本次不做。

## D4 空数组语义：显式 `[]` = 清空基线

已验证当前行为（代码 + 测试双重证据）：容器级 `#[serde(default)]` 的语义是"缺席键从结构体 `Default` 取"，出现键（含 `[]`）整体覆盖。测试证据：缺席 → 34 项（`config_missing_sections_default_missing_keys`，绿）；非空显式值 → 原样替换（`config_parses_all_sections`，绿）。显式 `[]` 走同一条"出现则覆盖"路径，结果只能是空 vec。

因此 `bootstrap_paths = []` + extend 即白名单模式是合法设计，无需改基线解析逻辑。但现有测试未直接 pin 住"显式 `[]` → 空"（是逻辑推论），本次必须补一个刻画测试（实现前后都应绿）。

## D5 保留 `Vec<String>`，否决 `Vec<Cow<'static, str>>`

- 可能编译不过：`Cow<'static, str>` 的 `Deserialize<'de>` 要求 `'de: 'static`，而 `load_config_from` 是 `toml::from_str(&content)`（局部 `String`）；改带生命周期泛型则 `Config` 逃不出函数（TOML 缓冲结束即 drop），文件来源条目无论如何只能 Owned。
- 即使绕过，收益为零：插件是按 action 启停的短命进程，配置加载每次一次，34 个小分配是单次微秒级；一次用户操作 spawn 若干 `jj` 子进程（毫秒级起）。火焰图顶点不在这里。
- 成本真实：所有消费点（sparse 拼接、校验报错、测试字面量、extend 新字段）都要感知 Cow，同构简单性丢失——而 extend 本来就必须是 `Vec<String>`。

## D6 校验与文档口径

- extend 条目空字符串 MUST 拒绝，错误定位到 `agent.extend_bootstrap_paths[i]`（与 `bootstrap_paths[i]` 同规则）；`~` 不展开（同规则）。
- 文档口径（放软半档，不说"不建议配 bootstrap_paths"）：95% 用户只需要 `extend_bootstrap_paths`；仅当需要**完全接管**列表（如排除默认项）时才手写 `bootstrap_paths`——后者是唯一的逃生口，不能堵死。
- README 示例改为一行可抄：`extend_bootstrap_paths = ["docs/AGENTS.md"]`。
- 命名定为 `extend_bootstrap_paths`（`extends` 心智，无强烈备选）。

## 测试策略

- 新增刻画测试：显式 `bootstrap_paths = []` → 生效基线为空（实现前后都绿；守护白名单前提）。
- 新增行为测试：extend-on-default（34+1）、extend-on-custom（自定义基线照样叠加）、空 extend 条目拒绝（定位到 `extend_bootstrap_paths[i]`）、去重保序。
- 既有 34 项 pin 测试与全部 85 项测试保持绿；`cargo build --release` 零警告。
