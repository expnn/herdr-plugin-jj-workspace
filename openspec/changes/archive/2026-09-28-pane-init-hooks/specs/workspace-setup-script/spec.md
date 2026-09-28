# workspace-setup-script Delta

## ADDED Requirements

（无）

## MODIFIED Requirements

### Requirement: pane 命令为单行脚本调用
wizard 生成的右侧 pane 命令 SHALL 为一行脚本调用（脚本绝对路径 + 逐参数 `shell_quote`），MUST NOT 包含拼接的 jj 命令序列、`sh_c_escape` 转义内容或对 pane shell PATH 的依赖。脚本路径 MUST 为绝对路径（基于 `HERDR_PLUGIN_ROOT`）。命令 MAY 携带来自 `[init]` 解析结果的可选前缀（`<init.right|init.default> && `，见 plugin-config 的 `[init]` 要求）：前缀为用户文本、原文注入、由 pane 的 shell 解析；无论是否携带前缀，脚本调用本身 MUST 仍是单次全参数引用调用，其参数形态 MUST NOT 因前缀存在而改变。

#### Scenario: 一行调用
- **WHEN** wizard 创建 jj 工作区且 `[init]` 未提供右侧非空命令
- **THEN** 右侧 pane 收到 `'<脚本绝对路径>' '<jj路径>' '<base-rev>' '<书签名>' '<ws>' '<t>' '<p>' [<'前导参数'>…]` 形态的单行命令，其中不含任何 `&&` 链或子 shell 组

#### Scenario: 携带 init 前缀
- **WHEN** wizard 创建 jj 工作区且 `init.right`（或 `init.default`）解析结果非空
- **THEN** 右侧 pane 收到 `<init 前缀> && '<脚本绝对路径>' '<jj路径>' …` 形态的单行命令，其中脚本调用部分与未配置时逐字节相同

## REMOVED Requirements

（无）
