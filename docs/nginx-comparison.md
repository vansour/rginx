# rginx 与 NGINX（开源版）对比

更新时间：`2026-04-10`

这份文档只比较：

- `rginx` 当前仓库实现与测试覆盖到的能力
- `NGINX OSS`，不把 `NGINX Plus / commercial subscription` 混入“开源版能力”

如果某个能力只在 NGINX Plus 里有，我会单独标出来，不算进开源版对等结论。

## 1. 比较目标

这份对比文档要回答三类问题：

1. `rginx` 已经覆盖了哪些 nginx 常见入口代理能力
2. `rginx` 还缺哪些 nginx 开源版能力，哪些是有意不做
3. 在双方都明确支持的场景里，性能应该如何做可重复、可解释的比较

## 2. 结论先看

如果只看“作为 HTTP/HTTPS 入口反代”的核心能力，`rginx` 已经不是 demo，而是一个有明确取舍的中型基础设施项目。

它当前更像：

- 一个聚焦型、带 typed config 和本地运维面的 Rust edge proxy
- 而不是一个要完整替代 nginx 全部生态面的 web server / gateway 平台

最重要的定位差异：

- `rginx` 强项在：`gRPC / grpc-web`、主动健康检查、本地 admin snapshot/delta、明确的 reload/restart boundary、相对收敛的配置模型
- `NGINX OSS` 强项在：成熟度、广泛部署、静态文件与通用 Web server 能力、模块与资料生态、长期性能与兼容性积累

## 3. 代码规模快照

当前仓库的一个粗略规模参考：

- `rginx` Rust 源码：约 `30,280` 行
- `rginx` Rust 测试：约 `13,614` 行
- `rginx` Rust 总计：约 `40,920` 行
- 官方 `nginx` 仓库 `src/*.c + *.h`：约 `246,379` 行

这说明 `rginx` 已经进入“中型基础设施项目”区间，但与 nginx 这种长期演化的老牌系统相比，体量和历史深度都还小很多。

## 4. 能力矩阵

状态说明：

- `Yes`：当前明确支持
- `Partial`：能做，但语义/方式不同，或需要额外前提
- `No`：当前不支持

| 维度 | rginx | NGINX OSS | 说明 |
| --- | --- | --- | --- |
| HTTP / HTTPS 反向代理 | Yes | Yes | 双方都覆盖主路径 |
| Host + Path 路由 | Yes | Yes | 双方都支持基于 `server_name + location` 的入口路由 |
| 静态文件服务 | No | Yes | `rginx` README 已明确当前阶段不以静态文件服务为目标 |
| WebSocket / HTTP Upgrade 透传 | Yes | Yes | `rginx` 已有 Upgrade 透传；NGINX 官方有 websocket proxying 文档 |
| gRPC 入口代理 | Yes | Yes | `rginx` 有基础 gRPC 代理；NGINX 有 `ngx_http_grpc_module` |
| grpc-web 转换 | Yes | No | `rginx` 原生做 binary / text 转换；官方 nginx 开源文档里未见原生 grpc-web 转换能力 |
| 下游 HTTP/2 | Yes | Yes | `rginx` 支持入站 HTTP/2；NGINX 有 `http2 on;` |
| 下游 mTLS 客户端证书校验 | Yes | Yes | `rginx` 支持 Optional / Required；NGINX 有 `ssl_verify_client`、`ssl_verify_depth`、`ssl_crl` |
| 上游 mTLS 客户端证书 | Yes | Yes | `rginx` 有 upstream client identity；NGINX 有 `proxy_ssl_certificate` / `proxy_ssl_certificate_key` |
| 上游证书校验深度 / CRL | Yes | Yes | `rginx` 有 verify depth / CRL；NGINX 有 `proxy_ssl_verify_depth` / `proxy_ssl_crl` |
| TLS 版本 / cipher / key exchange 控制 | Yes | Yes | 双方都可控，但具体粒度和表达方式不同 |
| OCSP stapling | Yes | Yes | 都支持，但实现模型不同：`rginx` 有基于证书 AIA + cache file 的运行时刷新；NGINX 官方是 `ssl_stapling*` 系列 |
| Round robin / least_conn / ip_hash | Yes | Yes | 双方都支持 |
| peer weight / backup | Yes | Yes | 双方都支持权重与 backup peer |
| 被动健康检查 | Yes | Yes | `rginx` 内建被动健康；NGINX 官方 load balancing 文档说明了 `max_fails` / `fail_timeout` |
| 主动健康检查 | Yes | No | `rginx` 支持 HTTP 和 gRPC active health；NGINX 官方 `ngx_http_upstream_hc_module` 标明为 commercial subscription |
| 幂等请求 failover | Yes | Partial | `rginx` 会基于方法和 replayable body 决定 failover；NGINX 有 `proxy_next_upstream` / `grpc_next_upstream`，但 buffering 与重试语义不同 |
| 自定义 access log 模板 | Yes | Yes | 双方都支持自定义 access log，但变量体系不同 |
| 本地 admin 面 | Yes | Partial | `rginx` 有本地 unix socket + snapshot/delta/wait；NGINX OSS 有 `stub_status`，但只有基础状态信息 |
| snapshot / delta / wait 运维接口 | Yes | No | 这是 `rginx` 当前一个明显差异点 |
| 配置 include | Yes | Yes | `rginx` 支持 `// @include` 和 `*.ron`；NGINX 有 `include` |
| 配置文件内环境变量展开 | Yes | No | `rginx` 支持 `${VAR}` / `${VAR:-default}`；NGINX 开源核心配置不提供同等 `${...}` 插值 |
| `nginx.conf` 兼容 | No | Yes | `rginx` 不是 nginx DSL drop-in replacement，但有 `migrate-nginx` 辅助 |
| `migrate-nginx` 工具 | Yes | N/A | 这是 `rginx` 自己的迁移工具能力 |
| `stream` TCP/UDP 代理 | No | Yes | `rginx` README 明确当前不做；NGINX 有 `stream` 模块体系 |
| FastCGI / uwsgi / SCGI / mail | No | Yes/Partial | 这类不是 `rginx` 当前目标；NGINX 生态完整得多 |
| 模块生态 | No | Yes | NGINX OSS 有长期积累的模块和资料生态；`rginx` 当前是更收敛的内建功能集合 |

## 5. 架构层面对比

### rginx

- Rust workspace，配置前端是 `load -> validate -> compile -> ConfigSnapshot`
- 单进程多 accept worker 运行时
- `SharedState` 统一承载配置快照、连接计数、流量统计、peer health、reload history、TLS runtime snapshot
- 本地 admin socket 直接暴露 `status / counters / traffic / peers / upstreams / snapshot / delta / wait`
- 明确建模 `reload boundary` 与 `restart boundary`

### NGINX OSS

- 经典 `master/worker` 架构
- 配置模型是 nginx DSL，模块把能力挂进统一配置树
- 观测与管理接口较分散，开源版主要是日志和基础状态模块
- 可做 live reload / live upgrade，但语义主要依赖成熟传统和模块行为，而不是像 `rginx` 这样在代码里先定义出 typed transition boundary

### 这意味着什么

- `rginx` 的优点是边界更显式，更容易做“知道自己改了什么、会不会跨 restart boundary”
- NGINX 的优点是成熟、广覆盖、生态强，很多问题在生产里已经被踩过无数次

## 6. 性能比较怎么做才公平

不要把“静态文件吞吐”当主结论。

原因很简单：

- `rginx` 当前阶段就不是静态文件服务器
- 如果用 nginx 最擅长而 `rginx` 明确不做的场景当 headline，会把结论带偏

更公平的第一阶段性能比较应该只测双方都明确支持、而且业务定位一致的场景：

1. `return 200` 纯入口开销
2. `HTTP/1.1` 反向代理到同一个上游
3. `TLS termination`
4. 入站 `HTTP/2`
5. `gRPC unary`
6. `reload` 延迟和连接排空
7. 内存占用、空闲连接占用、活动连接增长斜率

其中第 `1`、`2` 项最适合先落地为可重复脚本。

## 7. 现在仓库里的容器化性能方案

为了不污染宿主机，仓库里增加一套默认基于 `Debian trixie` 的 Docker 对比流程：

- Dockerfile: [docker/nginx-compare/Dockerfile](../docker/nginx-compare/Dockerfile)
- 容器内对比脚本: [scripts/nginx_compare.py](../scripts/nginx_compare.py)
- 宿主机包装脚本: [scripts/run-nginx-compare-docker.sh](../scripts/run-nginx-compare-docker.sh)

这套流程会：

1. 在 `trixie` 容器里构建 `rginx`
2. 在同一个容器里 clone 并构建最小 `nginx`
3. 跑这几类场景：
   - `return_200`
   - `proxy_http1`
   - `https_return_200`
   - `http2_tls_return_200`
   - `grpc_unary`
   - `grpc_web_binary` / `grpc_web_text`
   - `reload_return_body`
4. 用 `ab` 在同一容器内打压
5. 用并发 `curl` 在同一容器内跑 `TLS / HTTP2 / gRPC / grpc-web`
6. 把结果输出到：
   - `target/nginx-compare/performance-results.json`
   - `target/nginx-compare/performance-results.md`

运行方式：

```bash
./scripts/run-nginx-compare-docker.sh
```

如果你想调整压测规模：

```bash
./scripts/run-nginx-compare-docker.sh -- --requests 20000 --concurrency 128
```

当前这套脚本会把结果写到：

- `target/nginx-compare/performance-results.json`
- `target/nginx-compare/performance-results.md`

### 当前一次 trixie 容器 smoke 样本

这是我刚刚用默认 `trixie` Docker 环境跑的一次小样本：

- benchmark parameters: `40 requests / concurrency 4`
- `rginx`: `0.1.3-rc.6`
- `nginx`: `1.29.8`

| 场景 | nginx req/s | rginx req/s | rginx/nginx |
| --- | ---: | ---: | ---: |
| `return_200` | `41109.97` | `21470.75` | `0.522` |
| `proxy_http1` | `2044.47` | `100.45` | `0.049` |
| `https_return_200` | `210.80` | `88.61` | `0.420` |
| `http2_tls_return_200` | `221.86` | `102.64` | `0.463` |
| `grpc_unary` | `166.78` | `64.79` | `0.388` |

另外：

- `grpc_web_binary` 和 `grpc_web_text` 当前只跑 `rginx`
- `NGINX OSS` 在 harness 里被记录为 `unsupported`
- `reload_return_body` 这次样本里：
  - `nginx`: `31.847 ms`
  - `rginx`: `10.984 ms`

这组数字只能说明：

- 这套 Docker harness 现在已经能覆盖你要的六类比较入口
- 当前小样本里，`NGINX OSS` 在大多数已对齐场景上更快
- `grpc-web` 仍是 `rginx` 的差异化能力，不是双方对等对比项
- `reload` 现在也能形成可重复的时间指标
- 这些都还只是 smoke 级结果，不能直接拿去做对外宣传结论

如果要形成对外能站住的性能结论，至少还要补：

- 更大的请求量和更稳定的重复次数
- 多轮重复跑的统计汇总
- reload 期间旧连接排空而不只是新配置生效时间
- RSS / CPU / fd 使用曲线
- 更真实的上游后端，而不只是当前的最小 benchmark backend

## 8. 这套 benchmark 的边界

当前容器化脚本故意先收口，不追求“一口气测完所有协议”。

当前已覆盖：

- 纯 HTTP/1.1 `return 200`
- 纯 HTTP/1.1 reverse proxy
- 直接 `HTTPS`
- 直接 `HTTP/2 over TLS`
- `gRPC unary`
- `grpc-web binary/text`
- `reload` 生效时间

当前还没放进自动脚本，但建议下一步补：

- `PROXY protocol`
- `SIGHUP reload` 下的延迟抖动
- RSS / CPU / file descriptor / active connections 采样
- 更真实的 upstream，如 keepalive 池、多连接上游、TLS upstream verify on/off 对照

## 9. 如何解读结果

建议不要只盯着 `req/s`。

更有用的是同时看：

- 吞吐：`Requests per second`
- 平均延迟：`Time per request`
- 失败数：`Failed requests`
- 在同一个 workload 下的相对比值：`rginx/nginx`

并且要把结果绑定到具体 workload：

- 如果是 `return_200`，本质上测的是框架和请求路径开销
- 如果是 `proxy_http1`，本质上测的是入口代理实现和上游连接复用
- 以后补 `gRPC / grpc-web` 时，结果才更能体现 `rginx` 的差异化价值

## 10. 建议写进最终对外文档的结论口径

推荐用这种口径，而不是“谁全面更强”：

- `rginx` 不是 nginx 的全量替代品
- 在“中小规模部署的 HTTP/HTTPS 入口代理、API gateway 前置代理、gRPC / grpc-web ingress、主动健康检查、本地运维快照”这些收敛场景里，`rginx` 已经具备独立比较价值
- 在“静态文件服务、通用 Web server、多协议代理生态、模块生态、长期生产成熟度”这些维度，`NGINX OSS` 仍明显更成熟
- 如果比较双方，应该把“定位差异”和“功能面差异”写清楚，再谈性能

## 11. 参考资料

NGINX 官方文档，本文比较时主要参考：

- `Using nginx as HTTP load balancer`  
  https://nginx.org/en/docs/http/load_balancing.html
- `ngx_http_grpc_module`  
  https://nginx.org/en/docs/http/ngx_http_grpc_module.html
- `WebSocket proxying`  
  https://nginx.org/en/docs/http/websocket.html
- `ngx_http_v2_module`  
  https://nginx.org/en/docs/http/ngx_http_v2_module.html
- `ngx_http_ssl_module`  
  https://nginx.org/en/docs/http/ngx_http_ssl_module.html
- `ngx_http_proxy_module`  
  https://nginx.org/en/docs/http/ngx_http_proxy_module.html
- `ngx_http_realip_module`  
  https://nginx.org/en/docs/http/ngx_http_realip_module.html
- `ngx_http_stub_status_module`  
  https://nginx.org/en/docs/http/ngx_http_stub_status_module.html
- `ngx_http_upstream_hc_module`  
  https://nginx.org/en/docs/http/ngx_http_upstream_hc_module.html

仓库内部参考：

- [README.md](../README.md)
- [crates/rginx-http/src/transition.rs](../crates/rginx-http/src/transition.rs)
- [crates/rginx-runtime/src/admin.rs](../crates/rginx-runtime/src/admin.rs)
- [crates/rginx-http/src/proxy/forward.rs](../crates/rginx-http/src/proxy/forward.rs)
- [crates/rginx-http/src/state.rs](../crates/rginx-http/src/state.rs)
