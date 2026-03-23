# Roadmap and Gaps

本页是对顶层 [ROADMAP.md](../ROADMAP.md) 的 wiki 版摘要，方便在阅读其他页面时快速判断边界。

## 已经比较成型的能力

- HTTP/1.1 入站监听
- TLS 终止
- 入站 HTTP/2（TLS/ALPN）
- 基于 `Host` 的多虚拟主机
- `Exact` / `Prefix` 路由
- 反向代理
- WebSocket / Upgrade 透传
- upstream HTTPS
- upstream `round_robin` / `ip_hash` / `least_conn`
- peer `weight` / `backup`
- 被动和主动健康检查
- 路由级 ACL / 限流
- 静态文件、`HEAD`、单段 `Range`
- `/status` 与 `/metrics`
- `X-Request-ID`
- `SIGHUP` 热重载
- 平滑退出

## 当前明确缺口

### 架构类

- 多 worker
- 动态配置 API
- 配置 include
- 环境变量展开

### 协议与代理类

- 正则路由
- gRPC 代理完整语义
- Proxy Protocol
- upstream 明文 h2c

### 内容服务类

- Autoindex
- Gzip / Brotli

### 运维体验类

- 自定义 access log 格式
- 客户端侧超时治理

## 你应该如何理解“已支持”

这里的“已支持”更接近：

- 代码已经实现
- 仓库里有测试覆盖
- README/ROADMAP/wiki 可以说明清楚

它不等同于：

- 已经具备所有 Nginx 语义兼容性
- 已经覆盖所有边界输入
- 已经做过大规模生产打磨

## 近期建议优先级

如果目标是继续逼近 Nginx 的实用替代能力，建议优先做：

1. 客户端侧超时治理
2. access log 自定义格式
3. gRPC 代理能力
4. 压缩
5. 多 worker

原因：

- 这些能力对真实生产流量更敏感
- 比继续堆更多负载均衡变体更接近上线价值

## 哪些能力现在不建议优先做

在当前阶段，不建议把主要时间投入在：

- 复杂正则路由语法糖
- 很多细碎的配置兼容项
- 大量未配测试的 Nginx 行为模仿

更稳妥的策略是：

- 优先补“真实生产风险点”
- 每次新增能力同时补文档和端到端测试

## 参考

- 顶层对标表：[ROADMAP.md](../ROADMAP.md)
- 开发视角：[Development](Development.md)
