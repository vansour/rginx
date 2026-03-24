# rginx Wiki

`rginx` 的产品定义是：一个面向中小规模部署的 Rust 入口反向代理，稳定支持 TLS 终止、Host/Path 路由、上游代理、基础静态文件、基础流量治理、健康检查、热重载和可观测性。

当前正式发布线收口为 `v0.1.1`，当前正在准备下一条预发布标签 `v0.1.2-rc.1`。

这份 wiki 主要面向三类读者：

- 想先跑起来的使用者
- 想上线和运维的工程师
- 想继续开发功能的贡献者

## 先看哪份文档

仓库里的说明文档大致分三层：

| 文档 | 作用 |
| --- | --- |
| [README.md](../README.md) | 对外总览、能力清单、快速开始、配置结构、示例 |
| [ROADMAP.md](../ROADMAP.md) | 更细的能力矩阵、明确限制、工程演进观察和下一阶段建议 |
| `wiki/*.md` | 按主题展开说明：架构、配置、运维、开发、发布 |

如果三者出现冲突，优先级应是：

1. 代码与测试行为
2. Release Gate
3. README / ROADMAP / wiki

文档发现漂移时，应回到代码行为核对后直接回写文档，而不是继续累积“旧说法”。

## 阅读路径

### 如果你是第一次接触项目

建议按下面顺序读：

1. [Quick Start](Quick-Start.md)
2. [Configuration](Configuration.md)
3. [Examples](Examples.md)
4. [Routing and Handlers](Routing-and-Handlers.md)
5. [Upstreams](Upstreams.md)

### 如果你要评估当前支持边界

建议按下面顺序读：

1. [ROADMAP.md](../ROADMAP.md)
2. [Release Gate](Release-Gate.md)
3. [TLS and HTTP2](TLS-and-HTTP2.md)
4. [Operations](Operations.md)

### 如果你准备改代码

建议按下面顺序读：

1. [Architecture](Architecture.md)
2. [Development](Development.md)
3. [Roadmap and Gaps](Roadmap-and-Gaps.md)
4. [Refactor Plan](Refactor-Plan.md)

## 当前能力速览

下面这些能力当前已经比较成型：

- 单进程多 worker 运行时
- HTTP/1.1 入站监听
- HTTPS/TLS 终止
- 入站 HTTP/2（TLS/ALPN）
- 基于 `Host` 的默认虚拟主机和额外虚拟主机
- `Exact` / `Prefix` 路由匹配
- `grpc_service` / `grpc_method` 细分路由
- `Static` / `Proxy` / `File` / `Return` / `Status` / `Metrics` / `Config` 七种 handler
- 基础 gRPC over HTTP/2 代理
- 基础 grpc-web binary / text 转换
- `grpc-timeout` deadline 与本地代理错误到 `grpc-status` 的转换
- WebSocket / HTTP Upgrade 透传
- upstream `round_robin` / `ip_hash` / `least_conn`
- peer `weight` / `backup`
- upstream HTTPS、连接池、超时和 keepalive 调优
- 被动健康检查与主动健康检查
- 路由级 ACL 与限流
- 基础静态文件、`index` / `try_files` / `autoindex` / `HEAD` / 单段 `Range`
- br / gzip 基础响应压缩协商
- `trusted_proxies` 与 `X-Forwarded-For` 真实客户端 IP 解析
- `X-Request-ID` 透传或自动生成
- `/status` JSON 与 `/metrics` Prometheus 指标
- `Config` handler 动态配置 API（完整文档替换）
- 配置 `include` 与字符串环境变量展开
- `SIGHUP` 热重载
- `Ctrl-C` / `SIGTERM` 平滑退出
- `rginx check`

## 当前最重要的明确限制

下面这些点现在应被明确理解为“还不在稳定承诺内”：

- 不支持明文入站 HTTP/2（`h2c`）
- 不支持明文 upstream HTTP/2（`h2c`）
- 不支持正则路由
- 动态配置 API 只支持完整文档替换，不支持 patch
- 热重载不能切换 `listen`、`runtime.worker_threads` 或 `runtime.accept_workers`
- grpc-web 当前只覆盖基础 binary / text 模式，不承诺完整高级兼容
- Proxy Protocol 当前不支持

更完整说明见：

- [ROADMAP.md](../ROADMAP.md)
- [Release Gate](Release-Gate.md)

## Wiki 结构

| 页面 | 重点内容 |
| --- | --- |
| [Quick Start](Quick-Start.md) | 构建、启动、配置检查、第一条代理规则 |
| [Architecture](Architecture.md) | 工作区结构、运行时生命周期、请求链路、内部模块边界 |
| [Configuration](Configuration.md) | 顶层配置结构、字段语义、默认值与约束 |
| [Routing and Handlers](Routing-and-Handlers.md) | 虚拟主机、路由匹配、handler、文件服务、ACL、限流 |
| [Upstreams](Upstreams.md) | peer、负载均衡、权重、backup、超时、重试、健康检查 |
| [TLS and HTTP2](TLS-and-HTTP2.md) | 入站 TLS、SNI、ALPN、上游 TLS、HTTP/2、gRPC |
| [Operations](Operations.md) | 日常运维、状态页、指标、日志、热重载、信号 |
| [Examples](Examples.md) | 仓库内示例配置的用途和建议测试方式 |
| [Development](Development.md) | crate 分工、模块落点、测试体系、开发约束 |
| [Roadmap and Gaps](Roadmap-and-Gaps.md) | 对顶层 ROADMAP 的简版摘要 |
| [Release Gate](Release-Gate.md) | 当前稳定支持范围、非目标与发布闸门 |
| [Release Process](Release-Process.md) | 正式版发布流程 |
| [Refactor Plan](Refactor-Plan.md) | 历史与待续的代码结构拆分思路 |

## 同步说明

如果要把仓库内 `wiki/` 同步到 GitHub Wiki 仓库，可在仓库根目录运行：

```bash
./scripts/sync-wiki.sh
```

同步前建议先确认：

- README、ROADMAP、wiki 已经反映当前代码事实
- 没有把旧目录结构或旧能力状态继续写进页面
- 关键限制和发布边界仍与 [Release Gate](Release-Gate.md) 一致
