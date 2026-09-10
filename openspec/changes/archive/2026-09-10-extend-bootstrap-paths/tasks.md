# Tasks: extend-bootstrap-paths

## 1. 代码

- [x] 1.1 `AgentConfig` 新增 `extend_bootstrap_paths: Vec<String>`（默认空；`deny_unknown_fields` 下为已知键，老配置照常解析）
- [x] 1.2 生效清单计算：已解析 `bootstrap_paths` ++ extend，保序去重，集中到一处（如 `AgentConfig` 方法），sparse 调用点改用生效清单
- [x] 1.3 校验：extend 条目空字符串拒绝并定位 `agent.extend_bootstrap_paths[i]`；规则与 `bootstrap_paths` 一致（repo 相对，`~` 不展开）

## 2. 测试

- [x] 2.1 新增刻画测试：显式 `bootstrap_paths = []` → 生效基线为空（守护白名单前提；应独立于 extend 实现先绿）
- [x] 2.2 extend-on-default：未配基线 + extend 一项 → 34+1
- [x] 2.3 extend-on-custom：自定义基线 + extend → 两者并集（always-union）
- [x] 2.4 空 extend 条目拒绝（错误含 `extend_bootstrap_paths[1]` 定位）
- [x] 2.5 去重保序：extend 已有项只出现一次且顺序不变

## 3. 文档

- [x] 3.1 README：Configuration 示例改为一行可抄的 extend 示例；`agent.bootstrap_paths` 键说明补"95% 用户只用 extend"口径；新增 `agent.extend_bootstrap_paths` 键说明（含白名单模式）

## 4. 验证

- [x] 4.1 `cargo test` 全绿（含新增 5 项测试）
- [x] 4.2 `cargo build --release` 零警告
- [x] 4.3 既有 34 项 pin 测试未动且仍绿（默认值未被意外变更）
