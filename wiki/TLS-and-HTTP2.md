# TLS and HTTP2

本页说明 `rginx` 当前支持的 TLS 与 HTTP/2 能力，以及几个常见限制。

## 入站 TLS

开启方式：

```ron
server: ServerConfig(
    listen: "0.0.0.0:443",
    tls: Some(ServerTlsConfig(
        cert_path: "./certs/fullchain.pem",
        key_path: "./certs/privkey.pem",
    )),
),
```

要求：

- 证书和私钥必须是 PEM
- 路径相对配置文件目录解析

## SNI 与多证书

`rginx` 支持基于 SNI 选择不同证书。

来源：

- 默认虚拟主机的 `server.tls`
- 额外虚拟主机的 `servers[].tls`

匹配方式：

- 精确域名
- `*.example.com` 通配符

如果客户端的 `server_name` 没有命中额外证书，则回退到默认虚拟主机证书。

## 入站 HTTP/2

当前支持：

- TLS 上的 HTTP/2
- 通过 ALPN 在 `h2` 与 `http/1.1` 之间自动协商

当前不支持：

- 明文 `h2c`

## 上游协议选择

`UpstreamConfig.protocol` 支持：

- `Auto`
- `Http1`
- `Http2`

### `Auto`

- `https://` peer 会通过 TLS/ALPN 尝试协商 `h2`
- 如果没协商到，则回退为 HTTP/1.1

### `Http1`

- 强制使用 HTTP/1.1

### `Http2`

- 强制使用 HTTP/2
- 当前要求 peer 使用 `https://`
- 明文 `h2c` upstream 不支持

## 上游 TLS 模式

### `NativeRoots`

使用系统根证书校验 upstream 证书。

适合：

- 公网服务
- 证书链可被系统信任

### `CustomCa`

使用自定义 CA 文件：

```ron
tls: Some(CustomCa(
    ca_cert_path: "./certs/dev-ca.pem",
)),
```

适合：

- 内部 PKI
- 自建测试环境

### `Insecure`

跳过证书校验。

适合：

- 本地开发
- 临时联调

不建议：

- 生产环境

## `server_name_override`

当 upstream 的连接地址和证书域名不一致时，可以覆写 TLS SNI。

典型场景：

- 用 IP 直连上游
- 证书只签发给内部域名

## HTTP Upgrade / WebSocket

当前支持：

- HTTP/1.1 `Connection: Upgrade`
- WebSocket 透传

运行时行为：

- 建立 upgrade 后会启动双向字节流转发
- 连接关闭前，peer 的活跃请求数会一直保留

当前不支持：

- HTTP/2 extended CONNECT
- 完整 gRPC 语义代理

## 基础 gRPC over HTTP/2

当前支持：

- `application/grpc` 请求经 HTTP/2 upstream 透传
- 基础 `application/grpc-web(+proto)` / `application/grpc-web-text(+proto)` 请求可转换后转发到 HTTP/2 upstream
- 基于标准 `/grpc.health.v1.Health/Check` 的 gRPC 主动健康检查
- `TE: trailers` 请求头透传到 HTTP/2 upstream
- 下游 `grpc-timeout` 会参与 upstream 整体 deadline 计算，取其与 upstream `request_timeout` 的较小值；它同时约束等待响应头和后续响应 body 流；非法值会返回 `grpc-status = 3`
- 若下游在 gRPC / grpc-web 响应流结束前提前取消读取，且代理尚未观察到最终 `grpc-status`，则 access log 与 `rginx_grpc_responses_total` 会补记为 `grpc-status = 1`
- gRPC request trailer 透传到 HTTP/2 upstream
- grpc-web request trailer frame 可转换为 upstream HTTP/2 request trailers
- upstream 响应 body 与 trailing headers 透传给下游 HTTP/2 客户端
- upstream gRPC trailers 可编码回 grpc-web trailer frame，并在 text 模式下继续做 base64 编码后返回给下游客户端
- 当请求在代理本地失败时，`application/grpc` 会返回 trailers-only 风格的 `grpc-status` / `grpc-message`，grpc-web 会返回对应 trailer frame，而不是仅返回裸 HTTP 文本错误

当前仍不支持：

- 明文 `h2c` gRPC upstream
- 明文 `h2c` gRPC health probe
- 更高阶的 gRPC 语义，例如更主动的 cancellation 协同，或更完整的协议级兼容

## 常见限制

- 入站只支持 TLS 上的 HTTP/2，不支持 h2c
- grpc-web 当前只覆盖基础 binary/text 模式，不是完整的 grpc-web 语义代理
- upstream `Http2` 只支持 `https://` peer，经 TLS/ALPN 建链，不支持明文 h2c
- gRPC 主动健康检查同样只支持 `https://` peer，不支持明文 h2c
- 当前不是完整的 gRPC 语义代理实现

## 推荐阅读

- [Upstreams](Upstreams.md)
- [Operations](Operations.md)
