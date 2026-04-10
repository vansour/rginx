# rginx vs nginx Docker Trixie Smoke Snapshot

更新时间：`2026-04-10`

这是一份一次性的 benchmark 快照，保留当时的参数、样本结果和解释边界。

主文档只保留结论、方法和边界，不把这类一次性样本直接绑定到长期结论文案里。

## 环境

- Docker base image: `Debian trixie`
- benchmark parameters: `40 requests / concurrency 4`
- `rginx`: `0.1.3-rc.6`
- `nginx`: `1.29.8`

## 吞吐样本

| 场景 | nginx req/s | rginx req/s | rginx/nginx |
| --- | ---: | ---: | ---: |
| `return_200` | `41109.97` | `21470.75` | `0.522` |
| `proxy_http1` | `2044.47` | `100.45` | `0.049` |
| `https_return_200` | `210.80` | `88.61` | `0.420` |
| `http2_tls_return_200` | `221.86` | `102.64` | `0.463` |
| `grpc_unary` | `166.78` | `64.79` | `0.388` |

## 其他样本结果

- `grpc_web_binary` 和 `grpc_web_text` 当次只跑了 `rginx`
- `NGINX OSS` 在 harness 里被记录为 `unsupported`
- `reload_return_body`
  - `nginx`: `31.847 ms`
  - `rginx`: `10.984 ms`

## 如何解读这次样本

这组数字只能说明：

- 这套 Docker harness 已经能覆盖对齐后的主要比较入口
- 这次小样本里，`NGINX OSS` 在大多数已对齐场景上更快
- `grpc-web` 仍是 `rginx` 的差异化能力，不是双方对等对比项
- `reload` 已经能形成可重复的时间指标
- 这仍然只是 smoke 级结果，不能直接拿去做对外宣传结论

如果要形成对外能站住的性能结论，至少还要补：

- 更大的请求量和更稳定的重复次数
- 多轮重复跑的统计汇总
- reload 期间旧连接排空，而不只是新配置生效时间
- RSS / CPU / fd 使用曲线
- 更真实的上游后端，而不只是当前的最小 benchmark backend
