# rginx Codebase Modularization Policy

本文档定义仓库级“单文件单职责”约束。

配套 gate 脚本为：

- `scripts/run-modularization-gate.py`

模块布局与命名约定补充见：

- `docs/ARCHITECTURE_MODULE_LAYOUT_GUIDE.md`

基线文件位于：

- `scripts/modularization_baseline.json`

当前基线为空，只保留全局大小阈值；如果未来必须做临时豁免，也只能通过这个文件显式记录。

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
- 一句 `//!` 模块说明

不应继续演化成实现总装文件。

## 目录与命名约定

- 新增目录模块时，优先采用“门面 `mod.rs` + 职责子文件”的组织方式。
- 文件名优先表达职责，不使用模糊命名把多种逻辑继续堆叠到 `common.rs`、`misc.rs`、`utils.rs`。
- 如果某文件已接近 soft limit，后续功能默认继续拆子模块，而不是把 soft limit 当成新常态。

## 文件大小限制

### 生产文件

- soft limit: 300 行
- hard limit: 500 行

解释：

- 超过 soft limit 进入治理约束区。
- 新增 soft-limit 超限文件默认视为 gate 失败；只有历史基线中已有的 soft-limit 文件可以暂时保留，且不得继续增长。
- 超过 hard limit 需要拆分；仅允许出现在历史基线中，且不得继续增长。

### 测试文件

- soft limit: 400 行
- hard limit: 600 行

解释：

- 测试允许比生产代码更长，但仍应按场景拆分。
- 新增 soft-limit 超限测试文件默认视为 gate 失败；只有历史基线中已有的 soft-limit 文件可以暂时保留，且不得继续增长。
- 超过 hard limit 的历史测试文件可以暂时保留在基线中，但不得继续增长。

## 测试模块规则

- 生产文件中禁止新增内嵌 `mod tests { ... }`。
- 任何内嵌测试模块都只能出现在基线允许列表中。
- 遇到遗留内嵌测试时，默认迁移到独立测试文件。

说明：

- `#[cfg(test)] mod tests;` 外置测试声明目前不在 gate 范围内。
- gate 只禁止继续新增“内嵌测试实现块”。

## 基线文件

`scripts/modularization_baseline.json` 只用于记录临时豁免：

- 生产 soft-limit 超限文件及其当前最大行数
- 生产 hard-limit 超限文件及其当前最大行数
- 测试 soft-limit 超限文件及其当前最大行数
- 测试 hard-limit 超限文件及其当前最大行数
- 允许保留内嵌 `mod tests { ... }` 的生产文件

当前状态：

- 基线文件为空。
- 这意味着仓库当前没有正式纳管的模块化豁免项。

更新原则：

- 常规功能 PR 不得扩大基线
- 只有专门的模块化/拆分 PR 才应减少基线
- 若必须扩大基线，应视为架构例外，而不是默认做法

## Gate 行为

`scripts/run-modularization-gate.py` 会执行以下检查：

- 检查是否有新的生产文件超过 500 行
- 检查是否有新的生产文件超过 300 行
- 检查是否有新的测试文件超过 600 行
- 检查是否有新的测试文件超过 400 行
- 检查历史 soft-limit 文件是否比基线进一步增长
- 检查历史超限文件是否比基线进一步增长
- 检查是否有新的生产文件引入内嵌 `mod tests { ... }`

同时会输出以下非阻塞信息：

- 超过 soft limit 的生产文件列表
- 超过 soft limit 的测试文件列表
- 当前仍保留内嵌测试块的基线文件列表

## 执行入口

当前默认入口：

- `./scripts/test-fast.sh`
- `.github/workflows/ci.yml` 中的 `Modularization` job

因此本地快速回归和 PR CI 都会显式覆盖该 gate。
