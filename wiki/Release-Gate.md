# Release Gate

本页定义当前正式发布线 `v0.1.1` 的稳定支持范围、当前明确限制，以及进入正式版前必须满足的发布闸门。

这份文档的目标不是描述所有已实现代码，而是回答三个更具体的问题：

1. 哪些能力已经进入当前版本的稳定承诺。
2. 哪些能力当前明确不在稳定承诺内。
3. 发布正式版前，最少要验过哪些事情。

## `v0.1.1` 稳定支持范围

### 入口协议与 TLS

- HTTP/1.1 入站监听
- HTTPS/TLS 终止
- TLS/ALPN 协商的入站 HTTP/2
- 基于 SNI 的多证书选择

### Host / Path 路由

- 基于 `Host` 的默认虚拟主机和额外虚拟主机选择
- `server_names` 精确域名匹配
- `*.example.com` 通配符域名匹配
- `Exact("/foo")` / `Prefix("/api")` 两种路径匹配

### 上游代理

- `http://` 与 `https://` upstream
- `round_robin` / `ip_hash` / `least_conn`
- peer `weight` / `backup`
- 被动健康检查与主动健康检查
- `preserve_host` / `strip_prefix` / `proxy_set_headers`
- 幂等或可重放请求的 failover / retry
- HTTP/1.1 `Upgrade` / WebSocket 透传
- 上游 HTTPS + TLS/ALPN 协商到 HTTP/2

### 基础静态文件

- `File` handler
- `root`
- `index`
- `try_files`
- `HEAD`
- 单段 `Range`

### 基础流量治理

- 基于 CIDR 的 allow / deny
- 基于客户端 IP 的路由级限流
- `max_connections`
- `max_request_body_bytes`
- `header_read_timeout`
- `trusted_proxies` + `X-Forwarded-For` 真实客户端 IP 解析

### 健康检查、热重载与可观测性

- `rginx check` 配置检查
- `SIGHUP` 热重载
- `Ctrl-C` / `SIGTERM` 平滑退出
- `X-Request-ID` 透传或生成
- 结构化 access log / error log
- `/status` JSON 状态页
- `/metrics` Prometheus 指标页

## 当前明确限制与非目标

下面这些能力当前不应被视为 `v0.1.1` 的稳定承诺：

- 不支持明文入站 HTTP/2（`h2c`）
- 不支持明文 upstream HTTP/2（`h2c`）
- 热重载不能切换 `listen` 地址；变更监听地址必须重启
- 不支持正则路由
- 不支持完整的 gRPC 代理语义
- 不支持 Proxy Protocol
- 不支持 Autoindex
- 不支持 Gzip / Brotli 压缩
- 不支持自定义 access log 格式
- 客户端侧超时治理仍未补齐；当前稳定承诺只覆盖 `header_read_timeout` 和 upstream `connect/read/write/idle` 超时
- 不支持配置 include、环境变量展开、动态配置 API
- 当前不承诺多 worker / `SO_REUSEPORT` 架构
- 当前产品定位不是其他入口代理的 drop-in replacement，也不承诺语义级全面兼容

## 运维前提

`v0.1.1` 的稳定承诺默认建立在下面这些运维前提之上：

1. 发布前先执行 `rginx check`，不要把配置校验留到启动时碰运气。
2. 如果前面有 LB、CDN 或其他代理，`trusted_proxies` 必须配置准确，否则限流、ACL、`ip_hash` 和日志里的客户端 IP 都会失真。
3. `/status` 和 `/metrics` 应只暴露给内网、抓取器或受控运维入口，不应直接暴露公网。
4. upstream timeout、健康检查路径和健康检查阈值需要按你的业务特性做配置，不应完全依赖默认值。
5. 需要热重载时，只应修改可热更新的配置；涉及 `listen` 变化时必须走重启流程。
6. 进程生命周期默认由外部 supervisor 管理，例如 systemd、容器运行时或编排系统；仓库当前提供基础安装/卸载脚本，但不内置 systemd 或其他服务单元。

## 正式版发布闸门

只有当下面这些条件都成立时，才应切正式版 tag：

### 版本与文案

- 版本号已收口为稳定版，不再带 `-rc`
- README、wiki、CLI `about` 文案与当前产品定义一致
- 正式版对外描述只承诺本页列出的稳定支持范围
- `main` 只接受通过 PR 和 CI 的变更
- 正式版 tag 必须指向 `origin/main` 当前 HEAD，不能从分叉分支或历史旧提交直接切

### 自动化校验

必须通过：

```bash
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo run -p rginx -- --version
```

### 手工 smoke

至少要覆盖下面这些场景：

1. `rginx check --config ...` 成功，并输出合理的监听、vhost、route、upstream 摘要。
2. 一条基础代理路由可以正常回源。
3. 一条静态文件路由可以正常返回文件，且 `HEAD` / 单段 `Range` 行为正常。
4. 入站 TLS 可用，HTTP/2 可经 ALPN 正常协商。
5. `/status` 与 `/metrics` 可访问，且输出字段符合预期。
6. 被动或主动健康检查能驱动 failover。
7. `SIGHUP` 能在监听地址不变时热重载成功。
8. `Ctrl-C` 或 `SIGTERM` 能触发平滑退出。

### 发布产物

正式版发布前还应确认：

- GitHub Actions CI 通过
- tag 对应的 release workflow 通过
- 发布产物、`SHA256SUMS.txt` 和 Release notes 已生成
- Release notes 包含具体 changelog，而不是只有 tag 与 commit
- 仓库级 changelog 约定与实际 release notes 生成逻辑一致，见 [`/CHANGELOG.md`](../CHANGELOG.md)

## 如何使用这份文档

- 如果你在判断“这个能力能不能写进正式版对外承诺”，先看“稳定支持范围”。
- 如果你在判断“这个问题算 bug 还是未承诺能力”，先看“当前明确限制与非目标”。
- 如果你在判断“现在能不能切正式版”，直接按“正式版发布闸门”逐项核对。
