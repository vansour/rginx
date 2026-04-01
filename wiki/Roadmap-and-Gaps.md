# Roadmap and Gaps

本页是顶层 [ROADMAP.md](../ROADMAP.md) 的 wiki 版摘要，方便在阅读其他页面时快速判断：

- 哪些能力已经比较成型
- 哪些限制当前要明确接受
- 下一阶段最值得做什么

如果你需要更完整的能力矩阵、工程演进观察和阶段规划，请直接回到：

- [ROADMAP.md](../ROADMAP.md)

## 已经比较成型的能力

下面这些能力当前已经具备比较稳定的代码、测试和文档基础：

- 单进程多 worker 运行时
- HTTP/1.1 入站监听
- HTTPS/TLS 终止
- 入站 HTTP/2（TLS/ALPN）
- SNI 多证书
- 默认虚拟主机和额外虚拟主机
- `Exact` / `Prefix` 路由
- `grpc_service` / `grpc_method` 细分路由
- `Static` / `Proxy` / `File` / `Return` / `Status` / `Metrics` / `Config` handler
- 基础 gRPC over HTTP/2 代理
- 基础 grpc-web binary / text 转换
- `grpc-timeout` deadline
- 本地代理错误到 `grpc-status` 的转换
- 下游提前取消时的 `grpc-status = 1` 记账
- WebSocket / HTTP Upgrade 透传
- upstream `round_robin` / `ip_hash` / `least_conn`
- peer `weight` / `backup`
- 上游 HTTPS、连接池、超时和 keepalive 调优
- 被动健康检查与主动健康检查
- 路由级 ACL 与限流
- `trusted_proxies` 与 `X-Forwarded-For` 真实客户端 IP 解析
- 基础静态文件、`index` / `try_files` / `autoindex` / `HEAD` / 单段 `Range`
- br / gzip 基础响应压缩协商
- `/status` 与 `/metrics`
- `X-Request-ID`
- `Config` handler 动态配置 API（完整文档替换）
- 配置 `include` / 环境变量展开
- `rginx check`
- `SIGHUP` 热重载
- 平滑退出

## 当前明确限制

这些点当前仍然应明确视为限制，而不是“差一点就算支持”：

### 协议与代理

- 不支持明文入站 HTTP/2（`h2c`）
- 不支持明文 upstream HTTP/2（`h2c`）
- 不支持明文 `h2c` gRPC upstream
- 不支持 Proxy Protocol
- grpc-web 当前只承诺基础 binary / text 模式
- 不支持更完整高级 gRPC 语义兼容

### 路由与配置

- 不支持正则路由
- 动态配置 API 不支持 partial patch
- 热重载不能切换 `listen`
- 热重载不能切换 `runtime.worker_threads`
- 热重载不能切换 `runtime.accept_workers`

### 产品定位

- 当前目标不是其他成熟入口代理的全量兼容实现
- 当前优先级不是大量语法糖兼容，而是把现有稳定能力做扎实

## 当前工程状态

从内部结构看，最近已经完成了几件重要整理：

- 配置链路已经稳定分成 `load / validate / compile`
- HTTP 层已经从旧的单文件结构拆成 `handler/` 与 `proxy/`
- 集成测试层已经统一出共享 harness，降低了重复脚手架与 flaky 风险

但下面这些文件仍然偏大，后续可以继续按子领域拆：

- `crates/rginx-core/src/config.rs`
- `crates/rginx-config/src/validate.rs`
- `crates/rginx-http/src/proxy/health.rs`

## 推荐的下一阶段优先级

这里给的是“最自然、最值得”的建议顺序，不是版本承诺。

### 1. 继续做发布线硬化

优先级：高

建议内容：

- 继续补齐集成测试
- 继续减少 README / ROADMAP / wiki 与代码漂移
- 继续把测试脚手架统一到共享 harness

原因：

- 这类工作最直接影响上线可信度
- 比继续堆功能更能减少维护成本

### 2. 更完整的 gRPC / grpc-web 语义

优先级：高

建议内容：

- 更主动的 cancellation 协同
- 更细的错误分类与 observability
- 更多协议级端到端测试

原因：

- 当前 gRPC 已有不错基础，但仍是最复杂、最容易出现真实生产边界问题的区域

### 3. 更灵活的入口与匹配能力

优先级：中

建议内容：

- 正则路由
- 是否引入 Proxy Protocol
- 是否评估 `h2c`

原因：

- 这些能力确实有价值，但应晚于“协议硬化”和“运维硬化”

### 4. 配置管理与运维增强

优先级：中

建议内容：

- 更丰富的管理接口
- 动态配置 patch 能力评估
- 更完整的部署 / 运维文档与示例

## 哪些方向现在不建议优先做

当前阶段，不建议把主要精力投在下面这些方向：

- 没有真实场景驱动的大量兼容语法糖
- 没有测试覆盖的其他代理行为模仿
- 为了拆文件而拆文件的机械重构

更合理的顺序应该是：

1. 先修真实协议和运维风险
2. 再补配置与管理体验
3. 最后再做更广泛的兼容和语法扩展

## 开发时怎么使用这份页面

如果你在做功能判断，可以按下面方式使用：

- 判断“这个能力现在能不能对外承诺”：
  - 看 [Release Gate](Release-Gate.md)
- 判断“这个能力现在算支持、在建还是未支持”：
  - 看 [ROADMAP.md](../ROADMAP.md)
- 判断“这个能力应该改哪块代码”：
  - 看 [Development](Development.md)
- 判断“当前内部结构是不是已经有自然拆分方向”：
  - 看 [Architecture](Architecture.md) 和 [Refactor Plan](Refactor-Plan.md)

## 参考

- [ROADMAP.md](../ROADMAP.md)
- [Architecture](Architecture.md)
- [Development](Development.md)
- [Release Gate](Release-Gate.md)
