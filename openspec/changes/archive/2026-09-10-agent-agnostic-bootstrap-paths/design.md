# Design: agent-agnostic-bootstrap-paths

决策 D1-D4。调研基础：5 路并行 librarian 调研（2026-09-10），证据标准为官方文档 / 官方仓库一级来源，每条结论附 URL 与原文引用（调研结论另存于项目记忆 #162 / #163）。

## 调研范围

15 个主流编码 agent：Claude Code、Codex、Cursor、Windsurf（Cognition/Devin）、GitHub Copilot、Gemini CLI、Qwen Code、OpenCode、Amp、Goose、Crush、Cline、Kilo Code、Augment Code、Continue.dev。另核查 Aider（不自动读任何指令文件）与 Roo Code（2026-05-15 官方关停），以及 `.agents/` 目录的官方依据（Agent Skills 开放标准）。

## D1 纳入标准：仅"启动时同步读取"的仓库级路径

bootstrap 的机制窗口决定纳入标准：wizard 同步执行 `jj sparse set --clear --add <defaults>`，右侧 pane 随即（数秒内）执行 `sparse set --clear --add .` 全量物化。因此：

- **纳入**：agent 在 t=0 同步读取的仓库根文件/目录——指令文件、规则目录、启动即生效的配置（如 `.mcp.json`："Servers connect when the session begins"）。
- **不纳入**：懒加载 / JIT 读取（子目录 `AGENTS.md`、frontmatter `globs`/`paths:` 触发规则、按需注入的 `<system-reminder>`）——全量物化后天然覆盖；全局路径（`~/...`）——仓库外，`bootstrap_paths` 为 repo 相对路径。

被否方案：枚举子目录级 `AGENTS.md`/`CLAUDE.md`（无法用精确路径枚举，且按 D1 不需要）。

## D2 读取矩阵（保留的 34 项）

### 根目录指令文件（8）

| 路径 | 读取方（官方来源） |
|---|---|
| `AGENTS.md` | Codex（learn.chatgpt.com/docs/agent-configuration/agents-md）、Cursor（cursor.com/docs/rules）、Windsurf/Devin（docs.devin.ai/desktop/cascade/agents-md）、Copilot（docs.github.com/en/copilot/reference/custom-instructions-support）、Qwen（qwenlm.github.io/qwen-code-docs …/features/memory）、OpenCode（opencode.ai/docs/rules）、Amp（ampcode.com/docs/customize/agents-md）、Goose（goose-docs.ai …/using-goosehints）、Crush（github.com/charmbracelet/crush）、Cline（docs.cline.bot/customization/cline-rules.md）、Kilo（kilo.ai/docs/customize/agents-md）、Augment（docs.augmentcode.com/setup-augment/guidelines）——事实跨 agent 标准 |
| `AGENT.md` | Amp、Kilo 的官方回退文件名；Devin CLI project-level 清单（docs.devin.ai/cli/extensibility/rules.md） |
| `AGENTS.override.md` | Codex 每层覆盖文件（"If `AGENTS.override.md` exists it wins"） |
| `CLAUDE.md` | Claude Code（code.claude.com/docs/en/memory）、Cursor（cursor.com/help/customization/rules）、OpenCode、Crush、Augment、Copilot cloud agent |
| `CLAUDE.local.md` | Claude Code（与 CLAUDE.md 一同加载，同目录内排在其后；应 gitignore） |
| `GEMINI.md` | Gemini CLI（geminicli.com/docs/cli/gemini-md/，默认文件名）、Crush、Copilot cloud agent |
| `QWEN.md` | Qwen Code（与 AGENTS.md 同级默认加载） |
| `CRUSH.md` | Crush |

`.local` 变体取舍：`AGENTS.local.md` 仅 Devin CLI 提及、CRUSH/GEMINI 的 `.local` 变体为个人不入库文件——均排除；`CLAUDE.local.md` 保留，因其为 Claude Code 官方一等公民文件且加载语义明确（详见 D3）。

### 根目录工具专属文件（8）

| 路径 | 读取方 |
|---|---|
| `.mcp.json` | Claude Code（项目根，session 开始即连接） |
| `opencode.json` | OpenCode 启动配置（cwd → 向上至最近 git 目录；可含 `instructions` 引用） |
| `opencode.jsonc` | 同上（JSONC 形态） |
| `.cursorrules` | Cursor legacy（官方：仍读取、已官宣待弃用） |
| `.windsurfrules` | Windsurf legacy（官方：仍读取） |
| `.goosehints` | Goose（cwd 每层向上到 repo 根） |
| `.augment-guidelines` | Augment（仓库根） |
| `.github/copilot-instructions.md` | GitHub Copilot（repo 根，支持面最广：Chat/cloud agent/code review/CLI） |

### 目录（18）

| 路径 | 读取方与内容 |
|---|---|
| `.agents` | 跨工具技能目录 `.agents/skills/`：Codex（learn.chatgpt.com/docs/build-skills：REPO scope 自 cwd 扫描至 repo 根）、Amp、Goose、Crush 官方文档明示；Cline 官方源码（sdk …/storage/paths.ts `getWorkspaceSkillDirectories`）已含；依据 Agent Skills 开放标准实现指南（agentskills.io/client-implementation：".agents/skills/ paths have emerged as a widely-adopted convention"）。注意规范本身不强制路径，但官方指南明确推荐 |
| `.claude` | Claude Code：`.claude/CLAUDE.md`、settings.json、settings.local.json、rules/*.md、skills/、commands/、agents/ 等（hooks 只存在于 settings.json，**无** `.claude/hooks/` 目录）；Cline、Crush 亦读 `.claude/skills/` |
| `.codex` | Codex：config.toml、hooks.json、rules/*.rules（仅 trust 项目） |
| `.cursor` | Cursor：rules/*.mdc（无 frontmatter 的 `.md` 被忽略）、skills |
| `.gemini` | Gemini CLI：settings.json、commands/ |
| `.qwen` | Qwen Code：settings.json、skills/、QWEN.local.md |
| `.opencode` | OpenCode：agents/、commands/、modes/、plugins/、skills/、tools/、themes/ |
| `.windsurf` | Windsurf rules 回退目录（仍读取） |
| `.devin` | Windsurf/Devin 首选 rules 目录（Cognition 并购后） |
| `.clinerules` | Cline 主规则目录（内部所有 .md/.txt） |
| `.cline` | Cline 配置目录（rules/、skills/、hooks/、workflows/） |
| `.kilo` | Kilo Code 新规则目录（kilo.jsonc `instructions` 引用） |
| `.kilocode` | Kilo Code 官方向后兼容目录 |
| `.augment` | Augment rules 目录（仅 workspace 根，不递归） |
| `.continue` | Continue rules 目录——Continue **唯一**项目级入口（官方不支持 AGENTS.md，feature request continuedev/continue#6716 未合并） |
| `.github/instructions` | Copilot path-specific `*.instructions.md`（目录下任意层级） |
| `.crush` | Crush skills 目录（配置为 `.crushrc` **文件**——纯设置无指令内容，不入列） |
| `.goose` | Goose skills 向后兼容目录 |

## D3 排除清单

| 排除项 | 理由 |
|---|---|
| Roo Code 全套（`.roo/`、`.roorules`、`.roomodes` 等） | 扩展已于 2026-05-15 官方关停（docs.roocode.com 首页公告） |
| Aider 全套（`.aider.conf.yml`、`.env`、`.aider.model.*`、CONVENTIONS.md） | 不自动读任何指令文件（CONVENTIONS.md 需 `--read` 显式加载）；配置文件非指令；查找链 home→git 根→cwd 与本场景不匹配 |
| 各家 ignore 文件（`.geminiignore`、`.qwenignore`、`.clineignore`、`.aiderignore`、`.rooignore` 等） | 访问排除过滤器，非启动指令；需要者可自行配置 |
| `AGENTS.local.md`、CRUSH/GEMINI 的 `.local` 变体 | 个人不入库文件（base revision 中不存在，sparse 永远 no-op）；仅 Devin CLI / 单家提及。`CLAUDE.local.md` 例外保留：Claude Code 官方一等公民、加载顺序有明确定义 |
| 小写 `agents.md` | Windsurf 大小写不敏感是平台行为；Linux 文件系统大小写敏感，事实约定为大写 |
| 可配置文件名（Gemini/Qwen `context.fileName`、Codex `project_doc_fallback_filenames`、OpenCode `instructions` glob） | 用户自定义，无法枚举 |
| 子目录级 `AGENTS.md`/`CLAUDE.md` | 全部为懒加载 / JIT（Cursor IDE 子目录、Copilot nearest-wins、Kilo 动态注入、Claude 按需、Augment 层级发现）——D1 机制下天然覆盖 |
| 全局路径（`~/.claude/`、`~/.codex/`、`~/.gemini/` 等） | 仓库外；`bootstrap_paths` 为 repo 相对路径 |
| `tui.json`、`.continuerc.json` | 前者 OpenCode TUI 外观配置；后者 Continue 官方已废弃 |
| `.codex/skills/` | openai/codex 官方仓库全库检索零结果——Codex 仓库级技能只认 `.agents/skills/` |
| `.roo/workflows/` | 无官方证据（官方 `.roo/` 下仅 rules/、rules-{slug}/、commands/） |

## D4 纯超集原则

旧默认 4 项（`AGENTS.md`、`AGENTS.override.md`、`.codex`、`.agents`）全部保留于新清单——存量用户零回归。新增 30 项在不含对应文件的仓库中由 jj sparse 静默跳过（既有语义），并常驻 sparse 模式列表：仓库将来出现同名文件时自动物化。

被否方案：按 `agent.command` 的值选择性物化（`bootstrap_paths` 的跨 agent 单列表语义是 config-foundation D5 既定决策，不复议）。

## 测试策略

- 默认值 pin 测试：`config_missing_file_returns_defaults` 断言 34 项完整字面量——任何默认值变更必须显式过测试
- 常量类型标注 `[&str; 34]`：增删项导致编译错误，计数诚实
- `cargo test` 全绿 + `cargo build --release` 零警告
- 手工验收（可选，需真实 herdr）：在含 `CLAUDE.md` 与 `.cursor/rules/` 的仓库以默认配置创建工作区，确认启动阶段两者均已物化
