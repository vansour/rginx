# Upstream HTTP/3 Phase 0 Baseline

状态：归档文档。

这个文档记录上游 HTTP/3 专项启动时的基线判断。它回答的是：在开始把 upstream 侧做成生产可用之前，仓库已经有什么，缺什么，以及哪些约束不能破。

## 当时已有的基础

- 上游抽象已经支持显式 `protocol` 选择
- 已存在 upstream TLS 配置面：
  - 信任锚
  - 证书校验
  - SNI / 名称策略
  - client certificate
- 代理层已经有 request body buffering / replayability 判定
- 既有健康检查、peer 选择和管理面统计框架已经存在

## 阶段 0 缺口

- 没有生产级 HTTP/3 upstream client 路径
- 没有面向 HTTP/3 upstream 的 gRPC / grpc-web 代理覆盖
- 没有把 HTTP/3 upstream 纳入健康检查和发布门禁
- 没有对 reload / restart / control plane 的专项收口

## 关键约束

- upstream HTTP/3 必须显式 opt-in，不能默默替换已有 HTTP/1.1 / HTTP/2 行为
- 上游 TLS 名称策略必须延续现有配置语义，而不是重新定义一套字段
- failover 仍然必须受幂等 / 可重放约束控制
- 新能力必须落进仓库内集成测试，而不是只做手工演示

## 阶段 0 的成功标准

- 能把普通 HTTP 请求转发到 HTTP/3 upstream
- 能验证 TLS1.3、SNI 和 client identity 路径
- 后续可以在同一条基础链路上叠加 gRPC、health check 和观测能力

## 后续阶段如何消费这份基线

这份基线最终导向了几条长期约定：

- upstream HTTP/3 始终通过显式 `protocol: Http3` 开启
- 证书与身份相关行为跟随 upstream TLS 配置，而不是单独分叉
- HTTP/3 upstream 的发布门禁与主线 HTTP/3 gate 共用同一批高价值测试入口
