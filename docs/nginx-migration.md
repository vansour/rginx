# nginx Migration Guide

`rginx` 的迁移目标不是“完整兼容 nginx DSL”，而是尽快把常见的 HTTP / HTTPS API 反代入口迁到一个更收口、可验证的模型上。

这份文档覆盖的，是 Week 8 定义的最小迁移子集：

- `listen`
- `server_name`
- `location`
- `proxy_pass`
- `proxy_set_header`
- `client_max_body_size`
- `upstream server weight=...`
- `upstream server backup`

## 预处理

先把 nginx 的最终生效配置导出来，而不是直接喂带 `include` 的入口文件：

```bash
nginx -T > /tmp/nginx-expanded.conf
```

原因：

- 迁移工具不会递归执行 nginx 的 `include`、`map`、`geo`、`if` 语义
- `nginx -T` 会把常见站点片段先展开成一个可审阅的快照

## 迁移命令

仓库内置了一个最小迁移辅助子命令：

```bash
cargo run -p rginx -- migrate-nginx --input /tmp/nginx-expanded.conf --output /tmp/rginx.ron
```

生成后先做两步：

```bash
rginx check --config /tmp/rginx.ron
rginx -t --config /tmp/rginx.ron
```

`migrate-nginx` 的输出会在文件头部写出 warning 列表；这些 warning 不是噪声，而是需要人工复核的语义差异点。

## 指令映射

| nginx | rginx | 说明 |
| --- | --- | --- |
| `listen 80;` | `server.listen: "0.0.0.0:80"` | 纯端口写法会补成 `0.0.0.0:<port>` |
| 多个 `listen` | `listeners: []` | 需要人工确认 listener 与 vhost 的绑定语义 |
| `server_name api.example.com;` | `server.server_names` 或 `servers[].server_names` | 第一个 `server` block 会作为默认虚拟主机 |
| `client_max_body_size 10m;` | `server.max_request_body_bytes: Some(10485760)` | 如果 nginx 有多个值，迁移工具会提升到最大的 server 级限制 |
| `location /api { ... }` | `LocationConfig(matcher: Prefix("/api"), ...)` | 只迁移精确和前缀匹配 |
| `location = /healthz { ... }` | `LocationConfig(matcher: Exact("/healthz"), ...)` | `=` 语义会保留 |
| `proxy_pass http://backend;` | `handler: Proxy(upstream: "backend")` | `backend` 需要已声明的 `upstream` 或会被导成隐式 upstream |
| `proxy_set_header Host $host;` | `preserve_host: Some(true)` | 这是最常见、最可靠的 Host 保留映射 |
| `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;` | 不生成额外配置 | `rginx` 会自动写入干净的 `X-Forwarded-For` |
| `proxy_set_header X-Forwarded-Proto $scheme;` | 不生成额外配置 | `rginx` 会自动写入 `X-Forwarded-Proto` |
| `proxy_set_header X-Forwarded-Host $host;` | 不生成额外配置 | `rginx` 会自动写入 `X-Forwarded-Host` |
| `upstream backend { server 10.0.0.10:8080 weight=3; }` | `UpstreamPeerConfig(weight: 3)` | `backup` 同理会保留 |

## 当前限制

下面这些形态会直接触发 warning，或者要求人工处理：

- `location ~ ...` / `location ~* ...`
  - `rginx` 当前没有正则路由匹配器；必须手工改成 `Exact` / `Prefix`
- `proxy_pass` 带 URI path 或 query
  - 例如 `proxy_pass http://backend/internal;`
  - `rginx` upstream peer 只接受 `scheme://authority`，不会携带 path/query
- 变量型 `proxy_set_header`
  - 除 `Host $host` 和几条内建 `X-Forwarded-*` 之外，其它 `$variable` 会被跳过并产生命令头 warning
- location 级 `client_max_body_size`
  - `rginx` 当前只有 server/listener 级 `max_request_body_bytes`
- nginx `if` / `rewrite` / `map` / `geo` / `try_files`
  - 这些都不在当前迁移工具覆盖面内
- nginx `stream` / `mail` / unix socket listener
  - 不在当前版本线目标内

## 推荐迁移顺序

1. 先导出 `nginx -T` 快照。
2. 跑 `rginx migrate-nginx` 生成第一版 `RON`。
3. 先处理 warning，再跑 `rginx check`。
4. 把 nginx 里“默认自动补的头”删掉，尽量依赖 `rginx` 的内建行为。
5. 对 `proxy_pass` 带 path 的位置，手工改成：
   - `upstream` 只保留主机和端口
   - 路由层再决定是否加 `strip_prefix`
6. 迁移完成后，至少回归：
   - HTTP/1.1 反代
   - HTTPS / HTTP/2
   - gRPC / grpc-web
   - Upgrade / WebSocket
   - reload / restart

## 示例

nginx：

```nginx
http {
    upstream backend {
        server 10.0.0.10:8080 weight=3;
        server 10.0.0.11:8080 backup;
    }

    server {
        listen 8080;
        server_name api.example.com;
        client_max_body_size 8m;

        location /api {
            proxy_pass http://backend;
            proxy_set_header Host $host;
            proxy_set_header X-Static-Route api;
        }
    }
}
```

对应的 `rginx` 迁移结果会接近：

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 30,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:8080",
        server_names: ["api.example.com"],
        max_request_body_bytes: Some(8388608),
    ),
    upstreams: [
        UpstreamConfig(
            name: "backend",
            peers: [
                UpstreamPeerConfig(
                    url: "http://10.0.0.10:8080",
                    weight: 3,
                ),
                UpstreamPeerConfig(
                    url: "http://10.0.0.11:8080",
                    backup: true,
                ),
            ],
        ),
    ],
    locations: [
        LocationConfig(
            matcher: Prefix("/api"),
            handler: Proxy(
                upstream: "backend",
                preserve_host: Some(true),
                proxy_set_headers: {
                    "X-Static-Route": "api",
                },
            ),
        ),
    ],
    servers: [],
)
```

## 迁移后的必做项

- 补上 TLS 证书路径
- 按目标环境确认 `worker_threads`、`accept_workers`
- 补齐 `trusted_proxies`、`proxy_protocol`、hostname upstream、health check 等真实部署参数
- 在目标机器上跑一次 [benchmark 与 soak 基线](./benchmark-and-soak.md)
