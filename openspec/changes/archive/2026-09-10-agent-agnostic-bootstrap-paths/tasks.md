# Tasks: agent-agnostic-bootstrap-paths

## 1. 默认值与代码

- [x] 1.1 `DEFAULT_BOOTSTRAP_PATHS` 扩至 34 项（分组注释：根指令文件 / 根工具文件 / 目录），类型标注 `[&str; 34]` 保持计数诚实
- [x] 1.2 默认值 pin 测试更新为 34 项字面量（防意外变更）
- [x] 1.3 README：Configuration 示例改为注释形式的覆盖示例；`agent.bootstrap_paths` 键说明补默认值覆盖范围与懒加载豁免；Quickstart "Codex's startup instructions" 措辞更新

## 2. 文档与 spec

- [x] 2.1 openspec 变更四件套（proposal / design / tasks / delta spec），design.md 含完整读取矩阵与排除理由
- [x] 2.2 `plugin-config` delta spec：三处 Requirement MODIFIED（默认值清单 + bootstrap 范围界定）+ 新增跨 agent 默认覆盖 scenario

## 3. 验证

- [x] 3.1 `cargo test` 全绿（85 passed）
- [x] 3.2 `cargo build --release` 零警告（exit 0）
- [x] 3.3 grep 完成判据：新常量命中 `AGENTS.override.md`（main.rs:159）与 `".agents"`（main.rs:175）；pin 测试命中（main.rs:2522/2536）；README 无 4 项旧默认示例残留（137/141 行为新 34 项说明文本）
