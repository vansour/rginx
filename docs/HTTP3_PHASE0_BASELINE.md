# HTTP/3 Phase 0 Baseline

状态：归档文档。

这个文档记录 HTTP/3 主线刚开始时的基线假设，用来解释后续阶段为什么能沿着现有架构推进，而不是重新搭一套独立运行时。

## 当时已有的基础

- 明确的多 crate 分层：
  - `rginx-config` 负责加载、校验、编译
  - `rginx-http` 负责数据面
  - `rginx-runtime` 负责 reload / restart / admin / health
- 已存在的 TLS 终止与 HTTP/2 入口模型
- 可复用的 `SharedState` / admin snapshot 机制
- Linux 下的 fd 继承式优雅重启能力
- 已经成型的集成测试 harness

## 阶段 0 的关键缺口

- 没有下游 HTTP/3 listener 能力
- 没有上游 HTTP/3 代理能力
- 没有 HTTP/3 下的 gRPC / grpc-web 路径
- 没有 HTTP/3 专属的控制面、指标和发布门禁

## 阶段 0 约束

- HTTP/3 必须走显式 listener 配置，而不是隐式启用
- 不能破坏现有 TLS、reload、restart 和 admin 模型
- HTTP/3 路线要复用既有路由、代理、状态与日志抽象
- 测试必须从一开始就落进仓库内，而不是只依赖外部演示环境

## 当时定义的最小成功标准

- 能建立下游 HTTP/3 ingress
- 能在 HTTP/3 下执行最基本的 `Return` / `Proxy`
- 运维面至少能感知 listener 级 HTTP/3 配置和运行状态
- 后续阶段可以在不推翻配置模型的前提下继续扩展

## 阶段 0 之后的结果

这些前提最终支撑了后续阶段：

- HTTP/3 listener 与现有 listener / vhost / route 模型对齐
- HTTP/3 管理面接入 `status` / `snapshot` / `traffic` / `counters`
- reload / restart / drain 语义延续到 HTTP/3
- HTTP/3 最终被纳入 fast/slow 测试层、TLS gate 和 release gate
