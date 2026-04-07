# Benchmark And Soak Baseline

Week 8 的目标不是给出“绝对性能承诺”，而是固定一套**可重复执行**的 benchmark / soak 基线，让发布和上线都不再只靠功能烟测。

## 场景矩阵

| 类型 | 场景 | 目标 | 仓库内入口 |
| --- | --- | --- | --- |
| Benchmark | HTTP/1.1 plain reverse proxy / return path | 建立非 TLS 请求的吞吐与平均时延基线 | `scripts/run-benchmark-matrix.py --http1-url ...` |
| Benchmark | TLS termination over HTTPS | 建立 TLS 握手与 HTTPS 请求基线 | `scripts/run-benchmark-matrix.py --https-url ...` |
| Benchmark | HTTP/2 over TLS | 建立 HTTP/2 入站基线 | `scripts/run-benchmark-matrix.py --http2-url ...` |
| Soak | HTTP/1.1 proxy path | 长时间重复回归基础反代 | `scripts/run-soak.sh` |
| Soak | TLS + HTTP/2 | 重复验证 ALPN / ingress h2 路径 | `scripts/run-soak.sh` |
| Soak | gRPC + grpc-web | 重复验证协议转换与 trailer 行为 | `scripts/run-soak.sh` |
| Soak | Upgrade / WebSocket | 验证 tunnel 透传稳定性 | `scripts/run-soak.sh` |
| Soak | reload / restart stability | 验证 listener fd 继承与 graceful drain | `scripts/run-soak.sh` |
| Soak | hostname upstream refresh | 验证 DNS 刷新路径 | `scripts/run-soak.sh` |
| Soak | inbound PROXY protocol | 验证真实地址链路 | `scripts/run-soak.sh` |

## Benchmark 驱动

仓库内置了一个最小 benchmark 驱动：

```bash
python3 scripts/run-benchmark-matrix.py \
  --http1-url http://127.0.0.1:18080/ \
  --https-url https://127.0.0.1:18443/ \
  --http2-url https://127.0.0.1:18443/ \
  --requests 400 \
  --concurrency 32
```

这个脚本的定位很刻意：

- 只依赖 `curl` 和 `python3`
- 默认测 end-to-end request throughput
- 指标只提供：
  - `requests`
  - `concurrency`
  - `elapsed_s`
  - `req_per_sec`
  - `avg_ms`

它不是替代专业压测器的最终方案，但足够把每次 release 前后的变化收敛到同一把尺子上。

## Soak 驱动

仓库内置的 soak 驱动会重复跑高信号集成测试：

```bash
./scripts/run-soak.sh --iterations 3
```

默认会覆盖：

- `phase1`
- `http2`
- `upstream_http2`
- `grpc_proxy`
- `upgrade`
- `reload`
- `dns_refresh`
- `proxy_protocol`

如果要更保守，可以在 release 前把迭代数提高到 `5` 或 `10`。

## 最新基线采样口径

正式发布前，至少保留下面这些信息：

| 字段 | 要求 |
| --- | --- |
| commit / tag | 对应的 Git SHA 或 release tag |
| config shape | 使用的是 return path、plain proxy 还是 TLS/h2 配置 |
| host shape | CPU / 核数 / 内核版本 / curl 版本 |
| benchmark requests / concurrency | 每次都固定 |
| soak iterations | 每次都固定 |
| 失败样本 | 直接附 stderr / CI 链接 |

建议把每次 release 候选的结果直接追加到这份文档，而不是散落在评论区。

## 当前基线快照

这份 Week 8 收口时已经记录了一次本地基线，口径如下：

- tag / commit: `v0.1.2-rc.7` / `feature/week8-migration-bench-release-closure`
- host shape: `16` vCPU, `Linux 6.12.74+deb13+1-cloud-amd64 x86_64`
- client tool: `curl 8.14.1` with `HTTP2`
- benchmark target: 本地双 listener `Return(200, "ok\n")` 配置
- benchmark requests / concurrency: `200 / 16`
- soak iterations: `1`

### Benchmark Snapshot

| scenario | requests | concurrency | elapsed_s | req_per_sec | avg_ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| http1_plain | 200 | 16 | 0.176 | 1138.14 | 13.11 |
| https_tls | 200 | 16 | 0.894 | 223.68 | 68.12 |
| http2_tls | 200 | 16 | 0.822 | 243.27 | 62.98 |

### Soak Snapshot

`./scripts/run-soak.sh --iterations 1` 已通过，覆盖：

- `phase1`
- `http2`
- `upstream_http2`
- `grpc_proxy`
- `upgrade`
- `reload`
- `dns_refresh`
- `proxy_protocol`

这组数字的意义是：

- 它们足够作为 release 前后的相对对比基线
- 它们不是跨机器、跨内核、跨证书算法的统一容量承诺
- 真正的上线容量仍然应在目标机器上重跑同一套脚本再定

## 当前容量边界

`rginx` 到 Week 8 为止，已经适合做中小规模入口反代，但还不该被包装成无限弹性的 drop-in replacement：

- 它是单实例单进程 runtime；横向扩容仍然依赖外部 LB / supervisor 编排
- HTTP/2 入站当前走 TLS/ALPN，不提供 cleartext h2c ingress
- `reload` 不能变更 `listen` / `runtime.worker_threads` / `runtime.accept_workers`
- 这些启动期结构变化要走 `restart`
- body limit 当前是 listener/server 级，不是 route 级
- PROXY protocol 当前只支持 inbound v1
- upstream peer 只接受 `scheme://authority`，不接受 path/query

这些边界不是“以后不做”，而是当前 release 文档里必须写清楚的真实约束。
