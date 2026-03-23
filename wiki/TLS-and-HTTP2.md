# TLS and HTTP2

本页说明 `Rginx` 当前支持的 TLS 与 HTTP/2 能力，以及几个常见限制。

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

`Rginx` 支持基于 SNI 选择不同证书。

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

## 常见限制

- 入站只支持 TLS 上的 HTTP/2，不支持 h2c
- upstream `Http2` 只支持 `https://` peer
- gRPC 所需的 trailer / streaming 语义尚未补齐

## 推荐阅读

- [Upstreams](Upstreams.md)
- [Operations](Operations.md)
