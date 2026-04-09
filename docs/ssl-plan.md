# SSL/TLS 完善计划

这份文档用于收敛 `rginx` 后续 SSL/TLS 能力建设计划。

目标不是逐字复刻 nginx/OpenSSL 的全部 `ssl_*` / `proxy_ssl_*` 指令，而是：

- 把当前已经可用的 TLS 子系统继续补到接近 nginx 常见生产能力
- 对 rustls 无法等价表达的能力明确标注为“不支持”或“近似支持”
- 保证所有新增 TLS 能力同时具备配置边界、运行时可观测性、测试覆盖和发布门槛

## 当前状态

当前代码已经具备一套可上线的核心 TLS 基础能力：

- 下游 TLS 终止
- 基于 SNI 的证书选择
- 通配符域名匹配
- 无 SNI 默认证书回退
- 下游 TLS 版本、ALPN、cipher suites、key exchange groups、session 相关策略
- 下游 mTLS `Optional` / `Required`
- 上游 HTTPS、自定义 CA、insecure、客户端证书
- 上游 `server_name` / `server_name_override`
- HTTPS upstream HTTP/2
- `check` / admin / access log / snapshot 对 TLS 的基础诊断

但还没有达到 nginx 全量 TLS 子系统的成熟度，差距主要集中在：

- 企业 PKI 长尾能力
- 吊销与动态 OCSP
- 更细粒度的 TLS policy
- peer 级或更细的上游 TLS 覆盖
- 更完整的热更新、回滚和运行时诊断
- 更多 nginx/OpenSSL 语义对齐

## 总体原则

- 先补正确性和边界，再补长尾能力
- 先补可观测性和可验证性，再补更多配置项
- 对不支持项显式报错，不做伪兼容
- 每个阶段都必须带测试、示例、`check` 诊断和 admin 可见性

## Phase 0：冻结目标边界

状态：

- 已完成

交付物：

- `docs/ssl-compat-matrix.md`

目标：

- 明确 TLS 能力要补到哪个级别，避免后续反复返工

工作项：

- 建立 nginx 常见 `ssl_*` / `proxy_ssl_*` 能力分类
- 区分已支持、可近似支持、明确不支持三类
- 固定 TLS 配置粒度边界
- listener / server 作为握手期 policy 层
- vhost 作为证书覆盖层
- upstream 当前作为 upstream 级 TLS 配置层
- 定义 TLS 发布门槛

完成标准：

- 后续每个 TLS 需求都能明确归类，不再靠临时判断

## Phase 1：下游证书与握手正确性补强

状态：

- 已完成

交付物：

- `docs/ssl-compat-matrix.md` 中下游 TLS 矩阵已冻结 Phase 1 的能力边界
- `rginx check` 已能输出证书、SNI、默认证书和过期摘要诊断
- 下游 TLS / SNI / 默认证书 / 证书链诊断已有单元测试和集成测试覆盖

本阶段已落地：

- 固定证书选择优先级：精确 SNI 优先于通配符 SNI，通配符按更具体后缀优先
- 修复 `*.example.com` 对根域 `example.com` 的误匹配
- 固定 `default_certificate` 语义：显式映射优先于 listener 默认 TLS 回退
- 支持无 SNI 客户端的默认证书回退
- 在证书加载阶段补了 cert/key 一致性校验，私钥不匹配会直接失败
- `check` 和运行时 TLS 快照已补证书指纹、链诊断、即将过期证书摘要、默认证书映射
- 证书链诊断已覆盖重复证书、链不完整、叶子证书异常、即将过期等基础问题
- 下游 TLS 握手失败已统一分类并进入 counters / admin / status 视图

验收证据：

- SNI 与默认证书：
  [tls.rs](/root/github/rginx/crates/rginx-http/src/tls.rs)
- 证书链诊断与过期摘要：
  [state.rs](/root/github/rginx/crates/rginx-http/src/state.rs)
- `check` TLS 诊断输出：
  [main.rs](/root/github/rginx/crates/rginx-app/src/main.rs)
- 相关测试：
  [tls.rs](/root/github/rginx/crates/rginx-http/src/tls.rs)
  [state.rs](/root/github/rginx/crates/rginx-http/src/state.rs)
  [check.rs](/root/github/rginx/crates/rginx-app/tests/check.rs)
  [tls_policy.rs](/root/github/rginx/crates/rginx-app/tests/tls_policy.rs)

目标：

- 把最影响上线正确性的证书和握手行为做扎实

工作项：

- 继续补强证书选择优先级
- 精确 SNI
- 通配符 SNI
- `default_certificate`
- 无 SNI 回退
- 多证书 bundle 选择
- 补更严格的证书材料校验
- cert/key 一致性
- 证书链结构
- 重复证书
- 即将过期证书
- 默认证书映射无效
- 统一 TLS 握手失败分类和 counters 语义

完成标准：

- SNI、默认证书、证书错配问题都能在 `check` 或启动前直接发现

## Phase 2：下游 TLS Policy 补齐到“生产可控”

状态：

- 已完成

交付物：

- 下游 TLS policy 已新增 `session_cache_size` / `session_ticket_count`
- 运行时 TLS listener 快照已暴露有效的 resumption / tickets / cache / ticket-count 状态
- TLS policy 校验、编译和运行态测试已覆盖新增配置项

本阶段已落地：

- `versions`、`cipher_suites`、`key_exchange_groups`、`alpn_protocols` 已稳定下沉到 rustls `ServerConfig`
- `session_resumption` / `session_tickets` 语义已补强，不再只停留在“有开关”
- 新增 `session_cache_size`
- `None` 时沿用 rustls 默认容量
- `Some(0)` 时关闭 stateful session cache
- `Some(n)` 时使用显式 server session cache 容量
- 新增 `session_ticket_count`
- 可显式控制发送的 TLS 1.3 ticket 数量
- 与 `session_tickets` / `session_resumption` 的组合关系已有显式校验
- 运行时 listener TLS 快照已输出有效值
- `session_resumption_enabled`
- `session_tickets_enabled`
- `session_cache_size`
- `session_ticket_count`

当前仍明确未支持：

- `ssl_session_timeout`
- ticket key rotation 周期显式配置
- 更细粒度的 session cache 共享策略
- OpenSSL 风格 `ssl_prefer_server_ciphers`

验收证据：

- 配置模型：
  [tls.rs](/root/github/rginx/crates/rginx-core/src/config/tls.rs)
  [model.rs](/root/github/rginx/crates/rginx-config/src/model.rs)
- 校验与编译：
  [validate/server.rs](/root/github/rginx/crates/rginx-config/src/validate/server.rs)
  [compile/server.rs](/root/github/rginx/crates/rginx-config/src/compile/server.rs)
- 运行时策略下沉：
  [tls.rs](/root/github/rginx/crates/rginx-http/src/tls.rs)
- 运行时快照：
  [snapshots.rs](/root/github/rginx/crates/rginx-http/src/state/snapshots.rs)
  [state.rs](/root/github/rginx/crates/rginx-http/src/state.rs)
- 相关测试：
  [compile.rs](/root/github/rginx/crates/rginx-config/src/compile.rs)
  [validate/tests.rs](/root/github/rginx/crates/rginx-config/src/validate/tests.rs)
  [tls.rs](/root/github/rginx/crates/rginx-http/src/tls.rs)

目标：

- 把 listener / server TLS policy 从“能配”提升到“生产可控”

工作项：

- 继续完善和收口现有 policy 面
- `versions`
- `alpn_protocols`
- `cipher_suites`
- `key_exchange_groups`
- `session_resumption`
- `session_tickets`
- 补 session / cache 相关可控项
- session cache timeout
- ticket key rotation 策略
- 更明确的 resumption 边界
- 对 nginx 常见但 rustls 只能近似表达的项给出明确语义

完成标准：

- 常见生产 TLS policy 可以配置、校验、解释

## Phase 3：下游 mTLS 企业能力补齐

状态：

- 已完成

交付物：

- 下游 mTLS 配置已新增 `verify_depth` / `crl_path`
- 下游 mTLS verifier 已支持证书链深度限制与 CRL-based revocation
- 请求上下文与 access log 已新增更细的客户端证书链信息
- 运行时 TLS listener 快照已暴露 mTLS verify depth 与 CRL 是否配置
- mTLS 握手失败已新增 `certificate_revoked` / `verify_depth_exceeded` 细分类

目标：

- 把 mTLS 从“有开关”提升到完整子系统

本阶段已落地：

- 新增 `ssl_verify_depth` 对应配置：
  `client_auth.verify_depth`
- 语义固定为“限制客户端提供的证书链深度（leaf + intermediates）”
- 新增 CRL-based client certificate revocation：
  `client_auth.crl_path`
- CRL 会在 TLS acceptor 构建时装载并参与客户端证书校验
- 扩展了客户端证书身份视图：
  leaf issuer
  serial number
  chain length
  chain subjects
- access log 新增：
  `tls_client_issuer`
  `tls_client_serial`
  `tls_client_chain_length`
  `tls_client_chain_subjects`
- 运行时 TLS listener 快照新增：
  `client_auth_verify_depth`
  `client_auth_crl_configured`
- mTLS 握手失败细分类新增：
  `certificate_revoked`
  `verify_depth_exceeded`

当前仍明确未支持：

- OCSP based client certificate revocation
- 更丰富的 revocation 策略控制（unknown status / CRL expiration policy）

验收证据：

- 配置模型：
  [tls.rs](/root/github/rginx/crates/rginx-core/src/config/tls.rs)
  [model.rs](/root/github/rginx/crates/rginx-config/src/model.rs)
- 编译与校验：
  [compile/server.rs](/root/github/rginx/crates/rginx-config/src/compile/server.rs)
  [validate/server.rs](/root/github/rginx/crates/rginx-config/src/validate/server.rs)
- verifier 与 CRL 装载：
  [tls.rs](/root/github/rginx/crates/rginx-http/src/tls.rs)
  [certificates.rs](/root/github/rginx/crates/rginx-http/src/tls/certificates.rs)
- 请求上下文与 access log：
  [client_ip.rs](/root/github/rginx/crates/rginx-http/src/client_ip.rs)
  [access_log.rs](/root/github/rginx/crates/rginx-http/src/handler/access_log.rs)
  [access_log.rs](/root/github/rginx/crates/rginx-core/src/config/access_log.rs)
- 运行时快照与 counters：
  [snapshots.rs](/root/github/rginx/crates/rginx-http/src/state/snapshots.rs)
  [counters.rs](/root/github/rginx/crates/rginx-http/src/state/counters.rs)
  [state.rs](/root/github/rginx/crates/rginx-http/src/state.rs)
- 相关测试：
  [downstream_mtls.rs](/root/github/rginx/crates/rginx-app/tests/downstream_mtls.rs)
  [server.rs](/root/github/rginx/crates/rginx-http/src/server.rs)
  [tests.rs](/root/github/rginx/crates/rginx-core/src/config/tests.rs)

完成标准：

- 客户端证书问题不需要抓包就能做第一轮定位

## Phase 4：上游 TLS 与 `proxy_ssl_*` 对齐

状态：

- 已完成

交付物：

- 上游 TLS 已新增 `verify_depth` / `crl_path`
- 上游 TLS profile 已进入 `status` / `snapshot` / `upstreams` 运行时视图
- 上游 SNI / `server_name_override`、上游 mTLS、gRPC health over TLS 已有集成测试覆盖

目标：

- 把 upstream TLS 做到接近 nginx 常见生产面

本阶段已落地：

- 新增 `proxy_ssl_verify_depth` 对应配置：
  `UpstreamTlsConfig.verify_depth`
- 新增上游 CRL-based revocation：
  `UpstreamTlsConfig.crl_path`
- 上游 `server_name` / `server_name_override` 语义已固定：
  - `server_name` 控制是否发送 SNI
  - `server_name_override` 同时作为显式 server name resolver 和证书校验目标
- 上游 TLS 失败分类已稳定进入运行时统计：
  - `unknown_ca`
  - `bad_certificate`
  - `certificate_revoked`
  - `verify_depth_exceeded`
- 主动健康检查继续复用同一套 `ProxyClients` 和 upstream TLS profile，不额外分叉 TLS 语义
- upstream TLS 粒度仍维持 upstream 级，没有提升到 peer 级；这是本阶段的明确结论，不再作为当前阶段 blocker

当前仍明确未支持：

- peer 级 TLS 覆盖
- 动态 OCSP / 在线 revocation 生命周期管理

验收证据：

- 配置模型：
  [upstream.rs](/root/github/rginx/crates/rginx-core/src/config/upstream.rs)
  [model.rs](/root/github/rginx/crates/rginx-config/src/model.rs)
- 校验与编译：
  [validate/upstream.rs](/root/github/rginx/crates/rginx-config/src/validate/upstream.rs)
  [compile/upstream.rs](/root/github/rginx/crates/rginx-config/src/compile/upstream.rs)
- 上游 TLS client 与失败分类：
  [clients.rs](/root/github/rginx/crates/rginx-http/src/proxy/clients.rs)
  [mod.rs](/root/github/rginx/crates/rginx-http/src/proxy/mod.rs)
  [forward.rs](/root/github/rginx/crates/rginx-http/src/proxy/forward.rs)
- 运行时快照与运维面：
  [state.rs](/root/github/rginx/crates/rginx-http/src/state.rs)
  [snapshots.rs](/root/github/rginx/crates/rginx-http/src/state/snapshots.rs)
  [admin_cli.rs](/root/github/rginx/crates/rginx-app/src/admin_cli.rs)
- 相关测试：
  [upstream_server_name.rs](/root/github/rginx/crates/rginx-app/tests/upstream_server_name.rs)
  [upstream_mtls.rs](/root/github/rginx/crates/rginx-app/tests/upstream_mtls.rs)
  [grpc_proxy.rs](/root/github/rginx/crates/rginx-app/tests/grpc_proxy.rs)
  [admin.rs](/root/github/rginx/crates/rginx-app/tests/admin.rs)

工作项：

- 补 `proxy_ssl_verify_depth`
- 巩固 `server_name` / `server_name_override` 语义
- 补更清晰的上游证书校验失败分类
- 评估配置粒度是否要从 upstream 级提升到 peer 级
- 保证主动健康检查和正常代理流量复用同一 TLS profile 语义

完成标准：

- HTTPS upstream、gRPC upstream、上游 mTLS 都做到可控、可诊断、可验证

## Phase 5：OCSP 与证书生命周期自动化

状态：

- 已完成

交付物：

- 下游证书已支持基于 AIA + `ocsp_staple_path` 的动态 OCSP 拉取、缓存和后台刷新
- `status` / `snapshot` / `check` 已暴露 OCSP staple 路径、responder URL、缓存装载状态、最近刷新时间、失败信息
- reload 成功后会记录 TLS 证书指纹切换摘要，便于验证证书轮换是否真的生效
- 现有证书链诊断继续扩展到证书生命周期运维面，用于识别过期、链不完整、AKI/SKI 失配等问题

目标：

- 从静态文件装载升级到证书生命周期管理

本阶段已落地：

- 动态 OCSP：
  - 从证书 AIA 读取 responder URL
  - 生成 OCSP request
  - 后台拉取 responder response
  - 写入 `ocsp_staple_path` 缓存文件
  - 自动重建 TLS acceptor 以应用新 staple
- stapling 状态诊断：
  - `check` 可看到 responder URL、缓存文件、自动刷新是否启用
  - `status` / `snapshot` 可看到缓存是否装载、大小、最近刷新时间、失败计数和最近错误
- 证书生命周期：
  - 继续暴露 expiry warning
  - 继续暴露链不完整、path length、EKU、AKI/SKI、重复证书等链诊断
  - reload 后记录证书指纹切换摘要，支持证书轮换后核对是否真正生效

当前边界：

- 仍然没有实现完整 OCSP response 语义解析与 nextUpdate 驱动调度；当前刷新周期为固定后台轮询
- 当前没有把 OCSP 生命周期扩展到上游证书校验面

验收证据：

- OCSP request / AIA 提取与证书装载：
  [certificates.rs](/root/github/rginx/crates/rginx-http/src/tls/certificates.rs)
- 运行时 TLS / OCSP 快照：
  [state.rs](/root/github/rginx/crates/rginx-http/src/state.rs)
  [snapshots.rs](/root/github/rginx/crates/rginx-http/src/state/snapshots.rs)
- OCSP 后台刷新任务：
  [ocsp.rs](/root/github/rginx/crates/rginx-runtime/src/ocsp.rs)
  [bootstrap.rs](/root/github/rginx/crates/rginx-runtime/src/bootstrap.rs)
- admin / check / status 运维面：
  [admin_cli.rs](/root/github/rginx/crates/rginx-app/src/admin_cli.rs)
  [main.rs](/root/github/rginx/crates/rginx-app/src/main.rs)
- 相关测试：
  [ocsp.rs](/root/github/rginx/crates/rginx-app/tests/ocsp.rs)
  [reload.rs](/root/github/rginx/crates/rginx-app/tests/reload.rs)

工作项：

- 补动态 OCSP
- 拉取
- 缓存
- 刷新
- stapling 状态诊断
- 补证书生命周期能力
- expiry warning
- reload 后证书切换验证
- 证书链冲突分析
- 将这些能力纳入 admin / status / check

完成标准：

- 证书快过期、OCSP 过期、链冲突等问题都能在线上无抓包定位

## Phase 6：热更新、运维面与回滚能力

状态：

- 已完成

交付物：

- TLS reload / restart 边界已固定，并在 `check` / `status` 中稳定暴露
- reload 失败时会明确记录“保留当前 active revision”的回滚语义
- `status` / `snapshot` / `check` 已暴露 listener 视图、vhost 绑定、SNI 绑定冲突、默认证书绑定、upstream TLS profile、最近 reload 结果
- reload 成功后会记录 TLS 证书切换摘要，失败后会记录保留的 revision，用于变更后验证与故障回退确认

目标：

- 让 TLS 变更不再是高风险黑盒操作

本阶段已落地：

- 固定 TLS reload / restart 边界：
  - `server.tls`
  - `listeners[].tls`
  - `servers[].tls`
  - `upstreams[].tls`
  - `upstreams[].server_name`
  - `upstreams[].server_name_override`
  允许 reload
- 以下字段仍要求 restart：
  - `listen`
  - `listeners`
  - `runtime.worker_threads`
  - `runtime.accept_workers`
- 失败回滚语义：
  - reload 失败不会替换当前 active config
  - `status` 中会记录 `last_reload_active_revision`
  - `status` 中会记录 `last_reload_rollback_revision`
- 运行时 TLS 视图：
  - listener TLS policy
  - 证书详情与链诊断
  - OCSP 缓存/刷新状态
  - vhost -> certificate binding
  - SNI binding / conflict
  - default certificate binding
  - upstream TLS profile
  - 最近 reload 成败与证书切换摘要
- 变更前/后验证链路：
  - `check` 输出 reloadable / restart-required 字段
  - `check` 输出 TLS binding / OCSP / 证书链视图
  - `status` 输出最近一次 reload 的 active revision、rollback revision 和证书切换摘要

当前边界：

- 仍然没有独立的“回滚命令”；当前回滚语义是 reload 失败后自动保留上一版 active config
- 仍然没有把 TLS 变更验证固化成单独的 workflow subcommand，当前主要通过 `check` + `status` + `snapshot` 组合完成

验收证据：

- TLS 运行时快照与 binding 视图：
  [state.rs](/root/github/rginx/crates/rginx-http/src/state.rs)
  [snapshots.rs](/root/github/rginx/crates/rginx-http/src/state/snapshots.rs)
- reload / rollback 状态记录：
  [reload.rs](/root/github/rginx/crates/rginx-runtime/src/reload.rs)
  [state.rs](/root/github/rginx/crates/rginx-http/src/state.rs)
- admin / check / status 运维面：
  [admin_cli.rs](/root/github/rginx/crates/rginx-app/src/admin_cli.rs)
  [main.rs](/root/github/rginx/crates/rginx-app/src/main.rs)
- 相关测试：
  [admin.rs](/root/github/rginx/crates/rginx-app/tests/admin.rs)
  [reload.rs](/root/github/rginx/crates/rginx-app/tests/reload.rs)

工作项：

- 彻底固定 TLS reload / restart 边界
- 明确哪些字段允许 reload
- 明确哪些字段必须 restart
- 失败时如何回滚
- 扩展 admin / status / snapshot TLS 运行时视图
- listener 视图
- vhost 证书绑定
- upstream TLS profile
- 最近 reload 成败
- 补统一的变更前检查与变更后验证链路

完成标准：

- TLS 上线、reload、restart、回滚都有明确操作面和失败语义

## Phase 7：测试矩阵、示例、迁移工具与发布门槛

状态：

- 已完成

交付物：

- TLS 集成测试矩阵已覆盖下游 TLS / mTLS、上游 TLS / mTLS / HTTP2 / SNI、OCSP、reload、admin、check、migrate-nginx
- `configs/` / `example/` 已补 TLS 示例说明，默认活跃配置也有最小 TLS 模板注释
- `migrate-nginx` 已补更多 `ssl_*` / `proxy_ssl_*` 映射，并对无法精确映射的指令输出更明确 warning
- TLS 专项 gate 已固定为 `scripts/run-tls-gate.sh`，并纳入 CI / Release workflow

目标：

- 防止后续 TLS 回归

本阶段已落地：

- 测试矩阵：
  - `tls_policy`
  - `downstream_mtls`
  - `upstream_mtls`
  - `upstream_http2`
  - `upstream_server_name`
  - `grpc_proxy`
  - `access_log`
  - `admin`
  - `check`
  - `reload`
  - `ocsp`
  - `migrate`
- 示例：
  - `example/rginx.ron` 提供完整 TLS 注释参考
  - `configs/rginx.ron` 提供最小启用模板
  - `configs/conf.d/default.ron` 提供 vhost TLS 覆盖模板
- 迁移工具：
  - 新增 `ssl_verify_depth`
  - 新增 `ssl_crl`
  - 新增 `ssl_session_tickets`
  - 新增 `ssl_stapling_file`
  - 新增 `proxy_ssl_verify_depth`
  - 新增 `proxy_ssl_crl`
  - 对 `ssl_stapling on` 这类不能直接等价落地的配置给出更明确 warning
- 发布门槛：
  - `scripts/run-tls-gate.sh`
  - CI workflow 调用该 gate
  - Release workflow 调用该 gate

验收证据：

- gate 脚本与 workflow：
  [run-tls-gate.sh](/root/github/rginx/scripts/run-tls-gate.sh)
  [ci.yml](/root/github/rginx/.github/workflows/ci.yml)
  [release.yml](/root/github/rginx/.github/workflows/release.yml)
- 迁移工具：
  [migrate_nginx.rs](/root/github/rginx/crates/rginx-app/src/migrate_nginx.rs)
  [migrate.rs](/root/github/rginx/crates/rginx-app/tests/migrate.rs)
- 示例配置：
  [configs/rginx.ron](/root/github/rginx/configs/rginx.ron)
  [configs/conf.d/default.ron](/root/github/rginx/configs/conf.d/default.ron)
  [example/rginx.ron](/root/github/rginx/example/rginx.ron)

工作项：

- 扩大集成测试矩阵
- 多证书
- 默认证书
- 无 SNI
- 下游 mTLS
- 上游 mTLS
- 上游 SNI on/off
- session / tickets
- reload / restart
- OCSP
- 更新 `example/`、`configs/` 中的 TLS 示例
- 增强 `migrate-nginx` 对 `ssl_*` / `proxy_ssl_*` 的映射或 warning
- 把 TLS 专项 gate 固定到 CI / Release

完成标准：

- 每个新增 TLS 能力都必须同时有测试、示例、文档和发布门槛

## 推荐执行顺序

1. Phase 0
2. Phase 1
3. Phase 2
4. Phase 3
5. Phase 4
6. Phase 5
7. Phase 6
8. Phase 7

原因：

- 不先固定边界和正确性，后面的 TLS feature 会持续返工
- 不先把 listener / server / vhost / upstream 的职责分清，最终会出现“配置能写但行为不可靠”的伪支持
- 不把运维面和测试矩阵做完，后面越补越容易把现有 TLS 打坏

## 目标定义

最终目标建议定义成三层：

- 常见生产行为对齐 nginx
- 配置语义尽量接近 nginx
- 对 OpenSSL 专属能力明确不支持或近似支持

这意味着：

- 目标不是逐字复刻 nginx 所有 TLS 指令
- 目标是把常见生产能力做成稳定、可测、可诊断、可运维的 TLS 子系统
