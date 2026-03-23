# Examples

仓库内已经放了一组示例配置。本页说明每份配置的定位，以及你应该如何用它们做验证。

## 示例列表

| 文件 | 用途 |
| --- | --- |
| `configs/rginx.ron` | 基础代理、状态页、指标页 |
| `configs/rginx-ip-hash-example.ron` | 客户端 IP 粘性转发 |
| `configs/rginx-least-conn-example.ron` | 最少连接数转发 |
| `configs/rginx-weighted-example.ron` | 加权流量分配 |
| `configs/rginx-backup-example.ron` | 主备 peer |
| `configs/rginx-https-example.ron` | 系统根证书校验的上游 HTTPS |
| `configs/rginx-https-custom-ca-example.ron` | 自定义 CA 的上游 HTTPS |
| `configs/rginx-https-insecure-example.ron` | 跳过证书校验的开发联调 |
| `configs/rginx-vhosts-example.ron` | 多虚拟主机与 Host 路由 |

## 运行方式

统一写法：

```bash
cargo run -p rginx -- --config <example-file>
```

例如：

```bash
cargo run -p rginx -- --config configs/rginx-weighted-example.ron
```

## 每份示例适合验证什么

### `configs/rginx.ron`

建议验证：

- 根路径静态响应
- `/status`
- `/metrics`
- 基础 `/api` 代理

### `configs/rginx-ip-hash-example.ron`

建议验证：

- 相同客户端 IP 是否稳定落到同一 peer
- 变更客户端 IP 后，是否会分散到不同 peer

### `configs/rginx-least-conn-example.ron`

建议验证：

- 某个 peer 存在慢请求时，新请求是否会更倾向另一台机器

### `configs/rginx-weighted-example.ron`

建议验证：

- 高权重 peer 是否承担更多请求

### `configs/rginx-backup-example.ron`

建议验证：

- 正常情况下是否只打主 peer
- 主 peer 超时或不健康后，是否切到 backup peer

### `configs/rginx-vhosts-example.ron`

建议验证：

- 不同 `Host` 是否路由到不同 vhost
- 通配符域名是否生效
- 已命中 vhost 但无 route 时是否返回 `404`

## 本地验证技巧

### 模拟不同客户端 IP

如果你已经把本机加入 `trusted_proxies`，可以手工传：

```bash
curl -H 'X-Forwarded-For: 198.51.100.10' http://127.0.0.1:8080/api/demo
```

### 看 peer 健康与活跃请求

```bash
curl http://127.0.0.1:8080/status
```

重点关注：

- `healthy`
- `active_requests`
- `weight`
- `backup`

### 看配置是否可加载

```bash
cargo run -p rginx -- check --config configs/rginx-backup-example.ron
```

## 建议的学习顺序

如果你想按复杂度逐步理解功能，建议顺序是：

1. `configs/rginx.ron`
2. `configs/rginx-vhosts-example.ron`
3. `configs/rginx-ip-hash-example.ron`
4. `configs/rginx-least-conn-example.ron`
5. `configs/rginx-weighted-example.ron`
6. `configs/rginx-backup-example.ron`
7. 三个 HTTPS 示例
