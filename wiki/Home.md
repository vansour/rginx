# Rginx Wiki

`Rginx` 的产品定义是：一个面向中小规模部署的 Rust 入口反向代理，稳定支持 TLS 终止、Host/Path 路由、上游代理、基础静态文件、基础流量治理、健康检查、热重载和可观测性。

当前正式发布线收口为 `v0.1.1`，`v0.1.1-rc.1` 与 `v0.1.1-rc.2` 仅作为该版本线的预发布验证版本。

这份 wiki 面向三类读者：

- 想先跑起来的使用者
- 想上线和运维的工程师
- 想继续开发功能的贡献者

## 你应该从哪里开始

如果你第一次接触项目，建议按下面的顺序阅读：

1. [Quick Start](Quick-Start.md)
2. [Configuration](Configuration.md)
3. [Routing and Handlers](Routing-and-Handlers.md)
4. [Upstreams](Upstreams.md)
5. [Operations](Operations.md)
6. [Release Gate](Release-Gate.md)

如果你要改代码，建议直接看：

1. [Architecture](Architecture.md)
2. [Development](Development.md)
3. [Roadmap and Gaps](Roadmap-and-Gaps.md)

## 当前能力速览

- HTTP/1.1 入站监听
- HTTPS/TLS 终止
- 入站 HTTP/2（TLS/ALPN）
- 基于 `Host` 的多虚拟主机路由
- `Exact` / `Prefix` 两种路径匹配
- `Static` / `Proxy` / `File` / `Return` / `Status` / `Metrics` 六种处理器
- upstream `round_robin` / `ip_hash` / `least_conn`
- peer `weight` / `backup`
- 被动健康检查与主动健康检查
- 幂等或可重放请求的 upstream failover
- WebSocket / HTTP Upgrade 透传
- 路由级 ACL 与限流
- `/status` JSON 状态页
- `/metrics` Prometheus 指标
- `rginx check` 配置检查
- `SIGHUP` 热重载
- `Ctrl-C` / `SIGTERM` 平滑退出

## Wiki 结构

| 页面 | 重点内容 |
| --- | --- |
| [Quick Start](Quick-Start.md) | 构建、启动、配置检查、第一条代理规则 |
| [Architecture](Architecture.md) | 工作区结构、运行时生命周期、请求链路 |
| [Configuration](Configuration.md) | 顶层配置结构、字段语义、默认值与约束 |
| [Routing and Handlers](Routing-and-Handlers.md) | 虚拟主机、路由匹配、处理器、文件服务、ACL、限流 |
| [Upstreams](Upstreams.md) | peer、负载均衡、权重、backup、超时、重试、健康检查 |
| [TLS and HTTP2](TLS-and-HTTP2.md) | 入站 TLS、SNI、ALPN、上游 TLS、HTTP/2 |
| [Operations](Operations.md) | 日常运维、状态页、指标、日志、热重载、信号 |
| [Release Gate](Release-Gate.md) | 当前稳定支持范围、明确限制和正式版发布闸门 |
| [Examples](Examples.md) | 仓库内示例配置的用途和建议测试方式 |
| [Development](Development.md) | crate 分工、测试体系、功能开发落点 |
| [Roadmap and Gaps](Roadmap-and-Gaps.md) | 已支持/未支持清单与下一阶段建议 |

## 事实来源

wiki 的内容以仓库内三类文件为准：

- 顶层说明：[README.md](../README.md)
- 对标与差距：[ROADMAP.md](../ROADMAP.md)
- 代码实现：`crates/*`

当 README、wiki 和代码出现冲突时，应以代码行为和测试结果为准，再回写文档。
