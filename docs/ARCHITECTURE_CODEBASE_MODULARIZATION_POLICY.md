# rginx Codebase Modularization Policy

本文档定义仓库级“单文件单职责”约束。

它是 `ARCHITECTURE_CODEBASE_MODULARIZATION_PLAN.md` 的阶段 0 落地规则，配套 gate 脚本为：

- `scripts/run-modularization-gate.py`

历史遗留债务基线记录在：

- `scripts/modularization_baseline.json`

## 适用范围

适用于 `crates/` 下所有 Rust 源文件。

文件分类规则如下：

- 生产文件：
  - 位于 `crates/*/src/`
  - 且不是 `tests.rs`
  - 且不在 `*/tests/` 目录内
- 测试文件：
  - 位于 `crates/*/tests/`
  - 或文件名为 `tests.rs`

## 文件职责规则

### 通用规则

- 一个 `.rs` 文件只承载一个明确模块职责。
- 一个目录可以承载一个模块及其子模块。
- 目录门面文件允许存在，但默认只做模块接线和重导出。

### `main.rs`

`main.rs` 只应承载：

- 进程入口
- 参数解析分发
- 运行时初始化分发

不应继续承载：

- 大段检查报告渲染
- 大段配置总结逻辑
- 大段协议或状态拼装逻辑

### `lib.rs`

`lib.rs` 只应承载：

- `mod` 声明
- `pub use`
- 极薄的 crate 级门面逻辑

不应继续承载重业务逻辑。

### `mod.rs`

`mod.rs` 只应承载：

- 子模块声明
- 局部重导出
- 极薄的模块门面逻辑

不应继续演化成实现总装文件。

## 文件大小限制

### 生产文件

- soft limit: 300 行
- hard limit: 500 行

解释：

- 超过 soft limit 进入 review 关注区。
- 超过 hard limit 需要拆分；仅允许出现在历史基线中，且不得继续增长。

### 测试文件

- soft limit: 400 行
- hard limit: 600 行

解释：

- 测试允许比生产代码更长，但仍应按场景拆分。
- 超过 hard limit 的历史测试文件可以暂时保留在基线中，但不得继续增长。

## 测试模块规则

- 生产文件中禁止新增内嵌 `mod tests { ... }`。
- 历史遗留的内嵌测试模块仅允许出现在基线允许列表中。
- 后续阶段会逐步迁移这些内嵌测试到独立测试文件。

说明：

- `#[cfg(test)] mod tests;` 外置测试声明目前不在本阶段 gate 范围内。
- 本阶段只禁止继续新增“内嵌测试实现块”。

## 历史债务基线

`scripts/modularization_baseline.json` 记录三类临时豁免：

- 历史生产超限文件及其当前最大行数
- 历史测试超限文件及其当前最大行数
- 历史允许存在的内嵌 `mod tests { ... }` 文件

基线用途：

- 防止仓库因为历史债务而无法通过 gate
- 同时阻止这些历史问题继续恶化

更新原则：

- 常规功能 PR 不得扩大基线
- 只有专门的模块化/拆分 PR 才应减少基线
- 若必须扩大基线，应视为架构例外，而不是默认做法

## Gate 行为

`scripts/run-modularization-gate.py` 会执行以下检查：

- 检查是否有新的生产文件超过 500 行
- 检查是否有新的测试文件超过 600 行
- 检查历史超限文件是否比基线进一步增长
- 检查是否有新的生产文件引入内嵌 `mod tests { ... }`

同时会输出以下非阻塞信息：

- 超过 soft limit 的生产文件列表
- 超过 soft limit 的测试文件列表
- 当前仍保留内嵌测试块的历史文件列表

## 执行入口

当前默认入口：

- `./scripts/test-fast.sh`

因此现有 CI 和 release verify 路径会自动覆盖该 gate。
