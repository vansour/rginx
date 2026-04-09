# TLS Compatibility Matrix

这份文档是 `docs/tls-roadmap.md` 中 Phase 0 的正式交付物。

目的有两个：

- 把 `rginx` 当前 TLS/SSL 能力盘点成一张可以持续维护的事实表
- 把“像 nginx 对齐”拆成明确的已支持、部分支持、未支持三类，而不是继续靠口头描述

## 状态定义

- `已支持`：当前代码已经具备可用实现，语义基本稳定
- `部分支持`：已有实现，但和 nginx 常见语义仍有边界、限制或明显缺口
- `未支持`：当前没有对应能力，或只能通过修改代码实现

## 当前架构边界

Phase 0 审核后，TLS 相关边界可以先固定为下面几条：

- 下游握手期策略目前是 listener / legacy `server` 级能力，不是 vhost 级能力
- vhost 级 TLS 已经显式建模为 `VirtualHostTlsConfig` / `VirtualHostTls`，本质上是“证书覆盖层”，不是完整 TLS policy 层
- 上游 TLS 当前是 upstream 级能力，不是 peer 级能力
- reload 当前只禁止启动边界字段变更，TLS 配置本身理论上允许热替换
- admin / status 当前只暴露 `tls_enabled` 这类粗粒度状态，没有证书级视图

对应代码入口：

- vhost 级 policy 限制：`crates/rginx-config/src/validate/vhost.rs`
- 下游 TLS acceptor 构建：`crates/rginx-http/src/tls.rs`
- 证书加载与 OCSP 装载：`crates/rginx-http/src/tls/certificates.rs`
- 上游 TLS client 构建：`crates/rginx-http/src/proxy/clients.rs`
- reload 边界校验：`crates/rginx-http/src/state/helpers.rs`

## 下游 TLS 矩阵

| nginx 常见能力 | 当前状态 | `rginx` 现状 | 主要限制 |
| --- | --- | --- | --- |
| `listen ... ssl` | 部分支持 | 通过 `server.tls`、`listeners[].tls` 或 TLS vhost 触发 TLS terminate | 没有单独的 `ssl on` 风格开关，语义靠是否配置 TLS 材料推导 |
| `ssl_certificate` / `ssl_certificate_key` | 已支持 | `ServerTlsConfig.cert_path` / `key_path` | 仅文件路径模式 |
| 同一站点多证书材料 | 已支持 | `additional_certificates` 可加载多份证书/私钥并按签名算法选择 | 仍缺更强的装载诊断与运维视图 |
| SNI 精确匹配 | 已支持 | 按 `server_name` 精确匹配证书 | 当前实现稳定 |
| SNI 通配匹配 | 部分支持 | 支持前导 `*.` 形式，且不会再把根域当成 wildcard 命中 | 目前只支持前导 `*.` 这一种通配模式 |
| 无 SNI 默认证书回退 | 已支持 | 显式 `default_certificate` 优先，其次 listener 默认 TLS，最后是单个 TLS vhost 隐式回退 | 仍缺证书到期、摘要等更深诊断 |
| `ssl_protocols` | 已支持 | `versions: Some([Tls12, Tls13])` | 仅 listener / legacy `server` 级；vhost 级被显式禁止 |
| ALPN / 下游 HTTP/2 协商 | 已支持 | `alpn_protocols`，默认 `h2` + `http/1.1` | 仅 listener / legacy `server` 级 |
| `ssl_client_certificate` + `ssl_verify_client optional|required` | 已支持 | `client_auth.mode` + `ca_cert_path` | 仅 listener / legacy `server` 级 |
| 客户端证书身份提取 | 已支持 | `subject` / `san_dns_names` 已进入请求上下文、access log 和 admin 统计视图 | 还没有更细的链式诊断信息 |
| `ssl_verify_depth` | 未支持 | 无配置项 | 仍需扩展 rustls client verifier 策略 |
| `ssl_stapling_file` 风格静态 OCSP | 部分支持 | `ocsp_staple_path` 可把静态文件装进 `CertifiedKey` | 没有 stapling 状态视图和一致性诊断 |
| `ssl_stapling` / `ssl_stapling_verify` 动态治理 | 未支持 | 无自动抓取、刷新、校验 | 仅静态文件装载 |
| `ssl_ciphers` | 部分支持 | `cipher_suites` 可按 rustls/aws-lc-rs 支持集合显式配置 | 不支持 nginx/OpenSSL 任意 cipher 字符串语法 |
| `ssl_prefer_server_ciphers` | 未支持 | 没有单独开关 | 当前没有暴露 `ignore_client_order` 语义 |
| `ssl_ecdh_curve` / groups 调优 | 部分支持 | `key_exchange_groups` 可显式配置 | 语义是 rustls groups，不是 OpenSSL 曲线字符串 |
| `ssl_session_cache` / `ssl_session_timeout` | 部分支持 | `session_resumption` 可开关整体恢复能力 | 还没有 cache 大小、共享 cache、timeout 配置 |
| `ssl_session_tickets` | 部分支持 | `session_tickets` 可开关 ticket 生成 | 没有 ticket key 轮换参数暴露 |
| vhost 级完整 TLS policy | 未支持 | `versions` / `alpn_protocols` / `client_auth` 在 vhost 级会被拒绝 | 这是当前最关键的配置边界缺口 |

## 上游 TLS 矩阵

| nginx 常见能力 | 当前状态 | `rginx` 现状 | 主要限制 |
| --- | --- | --- | --- |
| `proxy_pass https://...` | 已支持 | upstream peer 支持 `https://` | 当前稳定 |
| `proxy_ssl_verify on|off` | 已支持 | `NativeRoots` / `CustomCa` / `Insecure` | `off` 对应 `Insecure` |
| `proxy_ssl_trusted_certificate` | 已支持 | `CustomCa { ca_cert_path }` | 只支持单个 upstream 级 CA 配置 |
| `proxy_ssl_certificate` / `proxy_ssl_certificate_key` | 已支持 | `client_cert_path` / `client_key_path` | 当前是 upstream 级，不是 peer 级 |
| `proxy_ssl_protocols` | 已支持 | `UpstreamTlsConfig.versions` | 当前稳定 |
| `proxy_ssl_server_name on|off` | 已支持 | `server_name: Option<bool>`，默认启用；关闭时会禁用 rustls SNI 扩展 | 当前只在 upstream 级配置 |
| `proxy_ssl_name` | 已支持 | `server_name_override` 作为证书校验目标 / SNI 名称覆盖 | 当前字段名仍沿用 `server_name_override` |
| HTTPS upstream 上的 HTTP/2 | 已支持 | `protocol: Auto|Http2`，HTTPS + ALPN 可走 h2 | `Http2` 当前要求所有 peer 都是 `https://` |
| 明确强制上游 HTTP/1.1 | 已支持 | `protocol: Http1` | 当前稳定 |
| h2c cleartext upstream | 未支持 | 校验层显式拒绝 | 当前只支持 HTTPS 上游 HTTP/2 |
| gRPC over TLS upstream | 已支持 | 复用 upstream TLS client | 当前稳定 |
| gRPC 健康检查 over TLS | 已支持 | `health_check_grpc_service` 复用 upstream TLS client 行为 | 当前要求 `https://`，不支持 h2c health check |
| `proxy_ssl_verify_depth` | 未支持 | 无配置项 | 需要 verifier 策略扩展 |
| `proxy_ssl_crl` | 未支持 | 无配置项 | 当前没有吊销能力 |
| `proxy_ssl_ciphers` / curves | 未支持 | 无配置项 | 需要单独设计 rustls 可暴露能力 |
| peer 级 TLS 覆盖 | 未支持 | TLS profile 是 upstream 级 | 当前不能对单个 peer 单独设 CA / 证书 / SNI |

## 运行时与运维矩阵

| 运维能力 | 当前状态 | `rginx` 现状 | 主要限制 |
| --- | --- | --- | --- |
| TLS 配置热 reload | 已支持 | 代码路径会重建 TLS acceptor / upstream clients，`check` 已明确输出可 reload 的 TLS 字段清单 | 仍需扩大多证书 / OCSP / 更复杂策略组合的 reload 回归 |
| TLS 变更触发 restart 边界判定 | 已支持 | `check` 和 reload 错误信息都已统一输出 restart-boundary：`listen` / `listeners` / `runtime.worker_threads` / `runtime.accept_workers` | 边界仍然是结构字段级，不是更细的 socket 级分类 |
| `check` 证书文件存在性校验 | 已支持 | compile 阶段检查 cert/key/CA/OCSP 文件是否存在 | 还不校验证书即将过期、链不匹配、策略冲突 |
| `check` 证书与私钥一致性校验 | 已支持 | TLS 初始化阶段会校验证书与私钥是否匹配 | 目前还没有更细的链路分析 |
| `check` TLS 诊断汇总 | 部分支持 | 已输出 TLS profile 数、vhost 覆盖数、SNI 名称数、bundle 数、default_certificate 映射、reloadable TLS 字段、expiring certificate 摘要 | 还没有证书摘要、链冲突分析 |
| admin TLS / mTLS 视图 | 部分支持 | `status` / `snapshot` 已暴露 listener TLS、SNI 名称、证书路径、到期时间、OCSP 是否配置；`counters` 已暴露 mTLS 与握手失败统计 | 还没有证书摘要 / 指纹与动态 OCSP 状态 |
| access log TLS / mTLS 字段 | 已支持 | 已支持 `tls_client_authenticated`、`tls_client_subject`、`tls_client_san_dns_names`、`tls_version`、`tls_alpn` | 还没有更多握手细节字段 |
| mTLS 失败原因定位 | 部分支持 | 已有结构化握手失败分类和聚合计数 | 仍缺更细的证书链级诊断 |
| nginx SSL 配置迁移 | 未支持 | `migrate-nginx` 尚未系统处理 SSL 指令子集 | 这是 Phase 7 事项 |

## 当前测试证据

已经存在的 TLS 相关自动化覆盖：

- 下游 TLS policy：`crates/rginx-app/tests/tls_policy.rs`
- 下游 HTTP/2：`crates/rginx-app/tests/http2.rs`
- 下游 mTLS：`crates/rginx-app/tests/downstream_mtls.rs`
- 上游 mTLS：`crates/rginx-app/tests/upstream_mtls.rs`
- 上游 HTTPS + HTTP/2：`crates/rginx-app/tests/upstream_http2.rs`
- 上游 SNI / `proxy_ssl_name` / `proxy_ssl_server_name`：`crates/rginx-app/tests/upstream_server_name.rs`
- TLS access log / admin / check / reload：`crates/rginx-app/tests/access_log.rs`、`crates/rginx-app/tests/admin.rs`、`crates/rginx-app/tests/check.rs`、`crates/rginx-app/tests/reload.rs`
- nginx SSL 迁移：`crates/rginx-app/tests/migrate.rs`
- reload / restart 边界：`crates/rginx-app/tests/reload.rs`

这说明当前基础能力不算空白，但也说明最缺的是：

- TLS 配置边界文档化
- 证书和 SNI 的诊断与运维视图
- nginx 指令级对应关系的系统整理

## 优先级 Backlog

### P0

- 固定 listener / vhost TLS 分层边界，明确哪些字段永远不能在 vhost 生效
- 补齐证书选择、默认证书、通配域行为文档与诊断
- 为 `check` 增加证书错配、SNI 冲突、默认证书无效的报错

### P1

- 继续补 admin TLS 视图中的证书摘要、SNI 映射和到期时间
- 评估上游 `proxy_ssl_verify_depth` / CRL 的可实现边界
- 评估 peer 级 TLS profile 的必要性与建模方式

### P2

- 扩大 TLS 回归测试矩阵，覆盖 multi-cert、OCSP、更多 SNI 组合
- 补证书摘要 / 指纹和更细的证书链诊断
- 继续扩展 `migrate-nginx` 对更复杂 nginx/OpenSSL 指令的识别范围

## Phase 0 结论

Phase 0 已经可以下结论：

- `rginx` 不是“没有 TLS”，而是“TLS 基础能力已有，子系统边界和运维面仍未成型”
- 真正优先级最高的不是继续零散加字段，而是先做配置边界和证书/SNI 语义收敛
- 后续路线图应该继续按 `Phase 1 -> Phase 2 -> Phase 3` 的顺序推进，而不是先做长尾能力
