# rginx NGINX Alignment Roadmap

本文档记录 `rginx` 在保留 `RON`、强类型配置和本地控制面特色前提下，向 NGINX 行为语义靠拢的长期路线。

目标不是复制 NGINX 配置语法，而是优先对齐：

- `server_name` 与 `location` 匹配语义
- `proxy_pass`/URI 改写语义
- 限流、缓存、TLS session 等共享状态能力
- reload / restart / worker 相关运行时边界

明确保留的 `rginx` 差异：

- `RON` 配置格式
- 配置加载、校验、编译三段式管线
- 本地 admin socket 与 `snapshot` / `delta` / `wait` 控制面
- 单二进制集成的 ACME / OCSP / runtime 能力

## Phase 0

建立兼容性基线。

当前要求：

- 把现有 host / route 选择行为落成测试，而不是只停留在代码阅读结论
- 把“哪些差异是有意保留的”写入长期文档
- 为后续行为变更准备 targeted regression tests

当前状态：

- 已建立长期路线文档
- 已补 host / route 选择的 targeted tests，覆盖 wildcard host、preferred prefix 和 regex 选择边界

## Phase 1

先对齐请求匹配语义。

当前要求：

- `server_name` 支持更接近 NGINX 的 wildcard 族谱
- route matcher 支持 `preferred prefix` 这类会改变 regex 优先级的前缀匹配
- route 选择从“全局排序取最大”转为“exact / longest prefix / regex”语义

当前状态：

- 已支持 `.example.com`
- 已支持 trailing wildcard 形式 `mail.*`
- 已增加 `PreferredPrefix(...)` matcher，并在 route 选择中实现“preferred prefix 阻断 regex”的行为
- 已把 URI 规范化前置到 route 匹配和 upstream URI 构造路径

仍未完成：

- `server_name` regex 仍未进入运行时模型
- `proxy_pass` URI 替换仍未完全对齐 NGINX

## Phase 2

增强共享状态与运行时边界。

目标：

- 限流从进程内状态走向 shared-zone 语义
- TLS session / ticket / ticket key 生命周期更接近多 worker 共享模型
- reload / restart 边界继续清晰化

当前状态：

- 已把 route rate limiting 从纯进程内状态扩展到基于 `config_path` identity 的共享 limiter state
- Linux runtime 现在会优先使用 SHM-backed shared limiter document；共享状态不可用时回退到本地 limiter
- TLS session cache 在当前单进程多 accept worker 架构下已经通过共享 `TlsAcceptor` / `ServerConfig` 维持同一份 session storage
- restart / reload 边界继续沿用现有 listener inheritance 和 transition boundary 机制
- Phase 2 已完成

## Phase 3

补齐高频行为能力。

目标：

- upstream 失败窗口、连接上限、keepalive 语义更接近 NGINX
- `proxy_*` / `real_ip_*` / log variables 继续扩展
- cache lifecycle 与 invalidation 继续成熟

当前状态：

- 已新增 per-peer `max_conns` 语义，并接入 upstream peer selection
- 已为 access log 增加 `upstream_name`、`upstream_addr`、`upstream_status`、`upstream_response_time_ms`
- 代理成功路径现在会把 upstream 结果写入响应 extensions，供最终 access log 渲染使用
- Phase 3 已完成

## Phase 4

做性能与硬化收口。

目标：

- 热路径分配、锁竞争和 worker 共享状态的性能收敛
- 回归测试、soak、兼容性样例覆盖
- 明确哪些行为已经达到“NGINX-like”，哪些仍是 `rginx` 差异

当前状态：

- 已补 end-to-end 回归样例，覆盖 dot wildcard host、normalized request path、preferred prefix、`max_conns` failover 和 upstream access-log variables
- 已把 phase 1 到 phase 3 的新增行为全部压到真实 `rginx` 子进程测试路径中，而不只停留在单元测试
- 当前阶段目标按本路线图范围已完成
