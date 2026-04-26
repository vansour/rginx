# rginx Module Layout Guide

本文档定义长期模块布局约定，用于约束后续功能默认如何落位。

它不替代 `ARCHITECTURE_CODEBASE_MODULARIZATION_POLICY.md` 的 gate 规则，而是补充“文件应该怎么长、目录应该怎么组织、门面应该保留什么”。

## 目标

- 新功能默认落到现有目录模块，而不是回填到历史门面文件。
- 目录名称、文件名称、门面职责保持一致，降低 review 成本。
- 模块入口文件和实现文件的边界对新贡献者可预测。

## 推荐布局

### 门面目录模块

适用于一个领域已经包含多个职责分片时：

- `foo/mod.rs`
- `foo/types.rs`
- `foo/build.rs`
- `foo/validate.rs`
- `foo/tests.rs`

规则：

- `mod.rs` 只保留模块说明、`mod` 声明、`use` 聚合、少量 `pub use`。
- 跨职责实现拆到独立文件，不把 `mod.rs` 当作“继续追加几百行逻辑”的缓冲区。
- 如果只有一个实现文件，不要为了形式强行创建多层目录。

### 单文件叶子模块

适用于职责非常集中且预计不会继续分叉时：

- `timeout/body.rs`
- `compression/options.rs`
- `proxy/health/request.rs`

规则：

- 文件名直接表达职责，不使用 `misc.rs`、`utils.rs`、`common.rs` 之类模糊名字，除非该模块确实只承载共享基础类型。

## 命名约定

- 协调/编排层优先使用 `mod.rs`、`orchestrate.rs`、`attempt.rs`、`setup.rs`、`scheduler.rs`。
- 数据结构层优先使用 `types.rs`、`model.rs`、`state.rs`、`snapshot.rs`。
- 协议辅助层优先使用 `request.rs`、`response.rs`、`codec.rs`、`headers.rs`、`body.rs`。
- 校验/转换层优先使用 `validate.rs`、`compile.rs`、`build.rs`、`parse.rs`。
- 只在“共享基础设施”确实成立时使用 `common.rs`、`helpers.rs`、`support.rs`。

## 测试布局

- 叶子模块优先保留同级 `tests.rs` 或 `tests/` 目录。
- 大型测试按场景拆分，如 `basic.rs`、`timeout.rs`、`reload.rs`、`snapshot.rs`。
- 测试辅助构造器放到 `support/` 或 `helpers/`，不要继续塞回场景测试文件。

## 模块文档要求

- 目录门面文件应有一句 `//!` 模块说明，回答“这个模块负责什么”。
- 如果某个模块有明显的子域边界，优先在门面文件中维持稳定的导出面，而不是让调用方直接依赖深层文件路径。
- 高风险协议区至少要让门面文件能读出子模块分层，例如：
  - 接收循环
  - 连接生命周期
  - 请求/响应桥接
  - 状态机/快照
  - 编码/签名/校验

## 默认动作

- 新增功能先找现有目录模块的职责落点。
- 如果现有文件接近 soft limit，优先新增子文件而不是追加到原文件。
- 如果新增门面文件开始承载实现，下一次同领域改动应优先继续拆薄，而不是接受其重新长回去。
