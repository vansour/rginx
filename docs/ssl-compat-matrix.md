# SSL/TLS 兼容矩阵

这份文档是 `docs/ssl-plan.md` 中 Phase 0 的正式交付物。

目的有三个：

- 明确 `rginx` 当前 SSL/TLS 子系统的实际边界
- 把常见 nginx `ssl_*` / `proxy_ssl_*` 能力分成“已支持 / 近似支持 / 不支持”
- 为后续 Phase 1 之后的实现提供统一参照，避免继续靠临时判断推进

## 目标定义

这里的“对齐 nginx”不定义为：

- 逐字复刻 nginx / OpenSSL 的全部指令面

这里定义为三层目标：

- 常见生产行为尽量对齐 nginx
- 配置语义尽量接近 nginx
- 对 OpenSSL 专属或 rustls 无法等价表达的能力明确标成“不支持”或“近似支持”

## 状态定义

- 已支持：当前代码已经具备稳定实现，能够用于生产语义
- 近似支持：已有实现，但语义、粒度或运维面仍与 nginx 有明显边界
- 不支持：当前没有实现，或者只能通过改代码实现

## 当前架构边界

当前代码里的 TLS 分层边界先固定为以下几条：

- listener / legacy `server` 是下游握手期 TLS policy 层
- vhost TLS 是证书覆盖层，不是完整 TLS policy 层
- upstream TLS 当前是 upstream 级，不是 peer 级
- reload 当前只禁止启动边界字段变更，TLS 配置本身允许热替换
- admin / `status` / `snapshot` 已有 TLS 视图，但还不是完整证书生命周期控制面

这些边界对应的核心代码入口：

- 下游 TLS 配置编译：
  [compile/server.rs](/root/github/rginx/crates/rginx-config/src/compile/server.rs)
- 下游 TLS 校验：
  [validate/server.rs](/root/github/rginx/crates/rginx-config/src/validate/server.rs)
- vhost TLS 证书覆盖编译：
  [compile/vhost.rs](/root/github/rginx/crates/rginx-config/src/compile/vhost.rs)
- 下游 TLS acceptor 与 SNI：
  [tls.rs](/root/github/rginx/crates/rginx-http/src/tls.rs)
- 上游 TLS client：
  [clients.rs](/root/github/rginx/crates/rginx-http/src/proxy/clients.rs)
- reload 边界校验：
  [helpers.rs](/root/github/rginx/crates/rginx-http/src/state/helpers.rs)

## 下游 TLS 矩阵

| nginx 常见能力 | 当前状态 | `rginx` 现状 | 主要边界 |
| --- | --- | --- | --- |
| `listen ... ssl` | 近似支持 | 通过 `server.tls`、`listeners[].tls` 或 TLS vhost 触发 TLS terminate | 没有单独 `ssl on` 风格开关，语义由 TLS 材料是否存在推导 |
| `ssl_certificate` / `ssl_certificate_key` | 已支持 | `ServerTlsConfig.cert_path` / `key_path` | 当前是文件路径模式 |
| 多证书材料 | 已支持 | `additional_certificates` 可加载多份证书/私钥并按签名算法选择 | 运维面还缺更强诊断 |
| SNI 精确匹配 | 已支持 | 基于 `server_name` 精确匹配证书 | 当前实现稳定 |
| SNI 通配匹配 | 近似支持 | 支持前导 `*.` 形式，并拒绝把根域误当 wildcard 命中 | 仅支持前导 `*.` 这一种语法 |
| 无 SNI 默认证书回退 | 已支持 | 显式 `default_certificate` 优先，其次 listener 默认 TLS，最后单个 TLS vhost 隐式回退 | 还缺更完整证书生命周期视图 |
| `ssl_protocols` | 已支持 | `versions: Some([Tls12, Tls13])` | 仅 listener / legacy `server` 级；vhost 级被禁止 |
| ALPN / HTTP/2 协商 | 已支持 | `alpn_protocols`，默认 `h2` + `http/1.1` | 当前是 listener / server 级 |
| `ssl_ciphers` | 近似支持 | `cipher_suites` 可显式配置 rustls/aws-lc-rs 支持集合 | 不支持 nginx/OpenSSL cipher string 语法 |
| `ssl_ecdh_curve` / groups | 近似支持 | `key_exchange_groups` 可显式配置 | 语义是 rustls groups，不是 OpenSSL 曲线字符串 |
| `ssl_session_cache` / `ssl_session_timeout` | 近似支持 | 已支持 `session_resumption` 与 `session_cache_size`，可控制是否启用 stateful cache 以及 cache 容量 | 还没有 cache timeout、共享 cache 策略配置 |
| `ssl_session_tickets` | 近似支持 | 已支持 `session_tickets` 与 `session_ticket_count`，可控制是否启用 tickets 以及发送数量 | 还没有 ticket key 轮换周期配置 |
| `ssl_prefer_server_ciphers` | 不支持 | 没有单独开关 | 当前没有独立的 ignore-client-order 语义 |
| vhost 级完整 TLS policy | 不支持 | vhost 仅允许证书覆盖 | 这是当前最关键的配置边界之一 |

## 下游 mTLS 矩阵

| nginx 常见能力 | 当前状态 | `rginx` 现状 | 主要边界 |
| --- | --- | --- | --- |
| `ssl_client_certificate` | 已支持 | `client_auth.ca_cert_path` | 当前稳定 |
| `ssl_verify_client optional|required` | 已支持 | `Optional` / `Required` | 握手和请求期行为已有测试覆盖 |
| 客户端证书身份提取 | 已支持 | subject / issuer / serial / SAN DNS / chain length / chain subjects 已进入请求上下文和 access log | 当前仍以请求期可见性为主 |
| mTLS 失败分类 | 近似支持 | 已区分缺失证书 / unknown CA / bad certificate / certificate revoked / verify depth exceeded / other | 还没有更丰富的 revocation policy 细分 |
| `ssl_verify_depth` | 已支持 | `client_auth.verify_depth` 可限制客户端提供的证书链深度 | 语义固定为 leaf + intermediates 的 presented chain depth |
| CRL based client revocation | 已支持 | `client_auth.crl_path` 可装载 CRL 并参与客户端证书校验 | 当前先支持静态 CRL 文件 |
| OCSP based client revocation | 不支持 | 无配置项 | 当前仍未实现 |
| 更细的客户端证书链调试视图 | 已支持 | issuer / serial / chain subjects / chain length 已可进入 access log 和请求上下文 | admin 面仍以 listener 级配置与聚合统计为主 |

## 上游 TLS 矩阵

| nginx 常见能力 | 当前状态 | `rginx` 现状 | 主要边界 |
| --- | --- | --- | --- |
| `proxy_pass https://...` | 已支持 | upstream peer 支持 `https://` | 当前稳定 |
| `proxy_ssl_verify on|off` | 已支持 | `NativeRoots` / `CustomCa` / `Insecure` | `off` 对应 `Insecure` |
| `proxy_ssl_trusted_certificate` | 已支持 | `CustomCa { ca_cert_path }` | 当前是 upstream 级，不是 peer 级 |
| `proxy_ssl_certificate` / `proxy_ssl_certificate_key` | 已支持 | `client_cert_path` / `client_key_path` | 当前是 upstream 级 |
| `proxy_ssl_protocols` | 已支持 | `UpstreamTlsConfig.versions` | 当前稳定 |
| `proxy_ssl_server_name on|off` | 已支持 | `server_name: Option<bool>` | 已真实下沉到 rustls `enable_sni` |
| `proxy_ssl_name` | 已支持 | `server_name_override` | 当前语义与 SNI / 校验目标一致 |
| HTTPS upstream 上的 HTTP/2 | 已支持 | `protocol: Auto|Http2`，可走 ALPN h2 | `Http2` 当前要求所有 peer 都是 `https://` |
| h2c cleartext upstream | 不支持 | 校验层显式拒绝 | 目前只支持 HTTPS 上游 HTTP/2 |
| gRPC health check over TLS | 已支持 | 主动健康检查复用同一套 `ProxyClients` | 当前要求 `https://`，不支持 h2c health check |
| `proxy_ssl_verify_depth` | 已支持 | `UpstreamTlsConfig.verify_depth` | 当前是 upstream 级，不是 peer 级 |
| 上游 TLS 吊销 / CRL | 已支持 | `UpstreamTlsConfig.crl_path` 可装载静态 CRL 并参与上游证书校验 | 当前是静态文件模式，不含在线刷新 |
| peer 级 TLS 覆盖 | 不支持 | TLS profile 当前是 upstream 级 | 后续要决定是否扩展粒度 |

## 运行时与运维矩阵

| 运维能力 | 当前状态 | `rginx` 现状 | 主要边界 |
| --- | --- | --- | --- |
| TLS 配置热 reload | 已支持 | 会重建 TLS acceptor / upstream clients，并保留 revision 级快照状态 | 当前边界是字段级，不是更细 socket 级 |
| TLS 变更触发 restart-boundary 判定 | 已支持 | `listen` / `listeners` / `runtime.worker_threads` / `runtime.accept_workers` 会要求 restart | `check` / `status` / reload 失败文案已统一 |
| `check` 证书文件存在性校验 | 已支持 | compile 阶段检查 cert/key/CA/OCSP 文件 | 当前稳定 |
| `check` 证书与私钥一致性校验 | 已支持 | TLS 初始化阶段校验证书与私钥匹配 | 当前稳定 |
| `check` TLS 诊断汇总 | 已支持 | 已输出 TLS profile 数、vhost 覆盖数、SNI 名称数、bundle 数、默认证书映射、即将过期摘要、vhost binding、OCSP responder / cache / auto-refresh 状态 | 当前不解析完整 OCSP response 语义 |
| admin TLS / mTLS 视图 | 已支持 | `status` / `snapshot` 已暴露 listener TLS、证书路径、过期时间、vhost binding、SNI binding/conflict、默认映射、mTLS 统计，以及 OCSP 缓存/刷新状态 | 当前下游 OCSP 为主，上游证书生命周期视图仍有限 |
| reload 失败自动保留 active config | 已支持 | reload 失败时不会替换当前 active revision，并在 `status` 中暴露 `last_reload_active_revision` / `last_reload_rollback_revision` | 当前是自动保留上一版，不是显式回滚命令 |
| `migrate-nginx` 对 `ssl_*` / `proxy_ssl_*` 的迁移 | 近似支持 | 已覆盖基础证书、协议、`ssl_verify_depth`、`ssl_crl`、`ssl_session_tickets`、`ssl_stapling_file`、`proxy_ssl_verify_depth`、`proxy_ssl_crl`、`proxy_ssl_name`、`proxy_ssl_server_name` | 更复杂 TLS 指令仍可能只给 warning |
| TLS 专项 CI / Release gate | 已支持 | `scripts/run-tls-gate.sh` 已固定到 CI / Release workflow | gate 仍以测试矩阵为主，不包含更重的长期 soak |
| access log TLS / mTLS 字段 | 已支持 | 已支持 `tls_version`、`tls_alpn`、`tls_client_*` 等字段 | 当前已够用 |
| 自动 OCSP 状态探测 / 动态刷新 | 近似支持 | 当证书 AIA 暴露 OCSP responder 且配置了 `ocsp_staple_path` 时，会后台拉取、缓存、刷新并重建 TLS acceptor | 当前为固定轮询刷新，不解析 nextUpdate |
| 更细粒度证书链冲突分析 | 近似支持 | 已有链不完整、AKI/SKI 失配、path length、重复证书、leaf EKU 等诊断，并在 reload 后记录证书指纹切换摘要 | 还没有单独的“链冲突”专用对象模型 |

## TLS 配置粒度冻结结论

Phase 0 先冻结以下边界，不在后续阶段里来回摇摆：

- listener / server 持有握手期 TLS policy
- vhost 只持有证书覆盖层，不承载完整 policy
- upstream TLS 当前先维持 upstream 级
- peer 级 TLS 覆盖是否要做，留到 Phase 4 再决策

这样做的原因：

- 下游握手发生在路由前，vhost 级完整 policy 很容易出现“配置能写但实际无法稳定切换”的伪支持
- upstream 级先维持简单模型，能避免 Phase 4 之前把 client cache、health check、连接池一起打散

## Phase 0 发布门槛

后续任何 TLS 新能力进入发布前，至少要满足：

- 有配置模型和校验逻辑
- 有 `check` 诊断或明确错误信息
- 有 admin / status / snapshot 可见性，至少说明是否生效
- 有最小端到端测试
- 有 `example/` 或 `configs/` 示例
- 对 nginx/OpenSSL 无法等价表达的语义有明确限制说明

## Phase 0 结论

当前可以明确下结论：

- `rginx` 不是“没有 SSL/TLS”，而是“核心生产子集已有，企业 PKI 长尾和 OpenSSL 级语义还没补齐”
- 当前最优先的不是继续零散加字段，而是沿着 Phase 1 之后的顺序继续补“正确性、粒度和运维面”
- 后续判断标准不应该是“有没有对应字段”，而应该是“行为是否稳定、是否可测、是否可诊断”
