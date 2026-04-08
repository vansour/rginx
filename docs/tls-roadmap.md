# TLS Roadmap

这份文档记录 `rginx` 面向 nginx 常见使用方式的 SSL/TLS 补齐计划。

目标不是逐字复刻 nginx 的全部 `ssl_*` / `proxy_ssl_*` 指令语法，而是在当前 Rust / rustls 技术栈里，把常见生产能力做成可配置、可校验、可观测、可维护的 TLS 子系统；对于无法等价支持的能力，明确标注为“近似支持”或“不支持”。

## 总目标

- 补齐下游 TLS、上游 TLS、SNI、多证书、mTLS、OCSP、ALPN、HTTP/2 等核心能力
- 尽量对齐 nginx 的常见行为和配置语义
- 对 rustls 无法等价表达的能力给出明确边界，而不是伪兼容
- 让 TLS 问题在 `check`、日志、admin snapshot 中可诊断

## 当前判断

当前项目已经具备一部分不错的 TLS 基础：

- 下游服务端 TLS 终止
- 基于 SNI 的证书选择
- `*.example.com` 这类通配 host 匹配
- 下游 ALPN：`h2` / `http/1.1`
- 下游 TLS 版本限制
- 下游静态 OCSP staple 文件装载
- 下游 mTLS `Optional` / `Required`
- 上游 HTTPS 代理
- 上游自定义 CA / native roots / insecure
- 上游客户端证书
- 上游 `server_name_override`
- HTTPS 上游 HTTP/2

但如果目标是“像 nginx 一样能把 TLS 当成完整子系统管理”，仍有明显缺口：

- TLS 配置分层边界还需要进一步收敛
- listener 级与 vhost 级 TLS 语义仍需明确
- 证书装载、证书诊断、默认证书回退还需要补强
- 与 nginx 常见 SSL 指令的对照关系还没有系统化落文档
- 热更新、admin 视图、日志诊断还不够强

## 设计原则

- 先做能力盘点和边界定义，再补功能
- 先补“正确性”和“可诊断性”，再补长尾能力
- 优先行为对齐，不追求 OpenSSL 指令面逐字复制
- 对不支持项显式报错，不做语义含混的兼容
- 尽量维持现有配置的兼容路径

## Phase 0: 能力盘点与对齐矩阵

状态：

- 已完成

交付物：

- `docs/tls-compat-matrix.md`

目标：

- 先把“要对齐什么”定义清楚，不直接进入零散编码

工作内容：

- 建立 nginx SSL / `proxy_ssl_*` 指令对照表
- 覆盖下游 TLS、上游 TLS、SNI、多证书、mTLS、OCSP、ALPN、HTTP/2、session/tickets、健康检查相关 TLS
- 把每项标成三类：已支持、部分支持、未支持
- 明确 listener 握手期配置和 vhost / SNI 期配置的边界

交付物：

- TLS compatibility matrix
- 优先级 backlog

验收标准：

- 后续每个 TLS 需求都能在矩阵里定位，不再靠临时判断

## Phase 1: TLS 配置模型收敛

状态：

- 已完成

本阶段已落地：

- vhost 不再复用 `ServerTlsConfig`
- 新增显式的 `VirtualHostTlsConfig`，把 vhost TLS 语义固定为“证书覆盖层”
- 运行时 `VirtualHost` 也不再携带完整 `ServerTls` policy，而是独立的 vhost 证书材料结构
- 旧的 `servers[].tls: Some(ServerTlsConfig(...))` 仍可反序列化，但只接受证书字段；一旦携带 policy 字段会直接报错

目标：

- 固定 TLS 配置的分层边界，避免后续反复返工

工作内容：

- 重新定义哪些字段必须属于 listener / server
- 重新定义哪些字段允许属于 vhost
- 明确哪些字段只能做“证书覆盖”，不能做完整 TLS policy 覆盖
- 把“证书身份”和“TLS policy”拆开建模
- 统一 `validate`、`compile`、`core config` 中的语义和错误信息

重点说明：

- 如果某些 TLS 策略在同一个 listener 上无法按 SNI 真正切换，就显式限制，不做伪支持
- 用户应能从报错里看懂某个 TLS 字段为什么必须放在 listener，而不能放在 vhost

验收标准：

- 配置分层稳定
- 错误信息可解释
- 后续 TLS 功能补齐不需要再次重构配置边界

## Phase 2: 下游证书与 SNI 语义补齐

状态：

- 已完成

本阶段已落地：

- 固定证书选择优先级：精确 SNI 优先于通配符 SNI，通配符按“更具体的后缀”优先
- 修复 `*.example.com` 误匹配根域 `example.com` 的行为，向 nginx 常见语义对齐
- 固定 `default_certificate` 优先级：显式 `default_certificate` 优先于 listener 默认 TLS 回退
- 在证书加载阶段增加 cert/key 一致性校验，`check` 现在能直接报出证书与私钥不匹配
- `rginx check` 增加 TLS 诊断输出：listener TLS profile 数、vhost 证书覆盖数、SNI 名称数、证书 bundle 数、显式 default_certificate 映射

目标：

- 先把最影响上线正确性的证书选择和 SNI 行为做扎实

工作内容：

- 完善证书选择优先级：精确域名、通配域名、默认证书、无 SNI 回退
- 补强多证书链、附加证书、默认域名映射相关校验
- 加强证书、私钥、OCSP staple 文件的一致性检查
- 固定 `default_certificate` 语义
- 增强 `rginx check` 的 TLS 诊断输出

验收标准：

- SNI 选择行为稳定且可预测
- 证书错配、默认证书无效、SNI 冲突能在启动前被发现

## Phase 3: 下游 TLS Policy 补齐

状态：

- 已完成

本阶段已落地：

- `ServerTlsConfig` / `ServerTls` 新增 `cipher_suites`、`key_exchange_groups`、`session_resumption`、`session_tickets`
- 这些配置会真实下沉到 rustls `CryptoProvider` 与 `ServerConfig`，不是占位字段
- 当前策略项只允许放在 listener / server TLS，vhost 继续只保留证书覆盖语义
- 校验层会拒绝空列表、重复项、cipher/version 不兼容、`session_resumption = false` 同时开启 tickets 这类无效组合
- 已新增运行时测试，直接验证 rustls `ServerConfig` 里的 cipher suites、groups、resumption、tickets 已被正确修改

目标：

- 补齐 nginx 用户最常用的下游 TLS 策略项

工作内容：

- 继续完善协议版本控制
- 固定 ALPN 与 HTTP/2 协商行为
- 评估并暴露 rustls 可稳定支持的 cipher suites、groups、session tickets、session resumption 等能力
- 对 nginx 有但 rustls 不具备等价语义的项，显式报错或标注限制

边界说明：

- 这一阶段的目标是“生产可用的策略能力”，不是 OpenSSL 级完全复刻

验收标准：

- 常见生产 TLS policy 能配置、能校验、能解释

## Phase 4: 下游 mTLS 完善

状态：

- 已完成

本阶段已落地：

- 巩固了 `Optional` / `Required` 的握手期行为，现有端到端测试继续覆盖匿名客户端、认证客户端和拒绝路径
- 客户端证书身份已进入请求上下文、access log 格式变量和默认结构化访问日志
- admin `status` / `counters` / `snapshot` 已新增 mTLS 视图：配置了多少 mTLS listener、optional/required 分布、认证连接数、认证请求数、匿名请求数、握手失败分类计数
- TLS 握手失败现在会做结构化分类并计数：缺失客户端证书、未知 CA、坏证书、其他错误
- `tls_client_authenticated`、`tls_client_subject`、`tls_client_san_dns_names` 已可用于 access log 模板

当前仍明确未支持：

- `ssl_verify_depth` / 自定义 verify depth
- CRL / OCSP based client cert revocation
- 更细粒度的客户端证书链调试视图

目标：

- 把客户端证书校验做成完整能力，而不是只有开关

工作内容：

- 巩固 `Optional` / `Required` 的握手和请求期行为
- 补客户端证书身份在请求上下文、日志、admin 视图中的暴露
- 评估并补充 CA bundle、校验深度、吊销相关能力
- 增加 mTLS 失败的可观测性

验收标准：

- mTLS 失败原因可定位
- 客户端证书身份信息可被安全消费

## Phase 5: 上游 TLS 与 `proxy_ssl_*` 对齐

状态：

- 已完成

本阶段已落地：

- upstream 配置新增显式 `server_name: Option<bool>`，用于对齐 `proxy_ssl_server_name on|off`
- `server_name_override` 继续保留为上游证书校验目标 / SNI 名称覆盖，对齐 `proxy_ssl_name`
- `server_name` 开关已经真实下沉到 rustls `ClientConfig.enable_sni`，不是占位配置
- 正常代理流量和 active health check 继续复用同一套 `ProxyClients`，因此 TLS profile 行为保持一致
- 上游请求失败和 active health 失败日志已补充 TLS 相关字段：是否启用 SNI、名称覆盖、verify 模式
- 已新增端到端测试，直接验证 `server_name_override` 会发送上游 SNI，以及 `server_name: Some(false)` 会关闭 SNI

当前仍明确未支持：

- `proxy_ssl_verify_depth`
- peer 级 TLS 覆盖
- 上游 TLS 吊销 / CRL

目标：

- 把上游 TLS 作为独立主题系统补齐

工作内容：

- 对齐上游 TLS 常见能力：verify on/off、自定义 CA、客户端证书、TLS 版本、SNI、`server_name_override`、HTTP/2 upstream
- 统一主动健康检查和正常代理流量在 TLS 配置上的行为
- 增强上游 TLS 握手失败、证书校验失败、SNI 失败的日志和统计
- 明确 upstream TLS 配置粒度到底是 upstream 级还是 peer 级

验收标准：

- HTTPS / gRPC / mTLS upstream 场景从“能连”提升到“可控、可诊断、可验证”

## Phase 6: 热更新、运维面与可观测性

状态：

- 已完成

本阶段已落地：

- `check` 现在会明确输出 TLS reload 边界：哪些 TLS 字段允许 `reload`，哪些字段属于 restart-boundary
- `check` 现在会输出证书即将过期摘要；当前阈值为 30 天
- `RuntimeStatusSnapshot` / admin `status` / admin `snapshot` 已新增 TLS 运行时视图：listener TLS 配置、SNI 名称、证书路径、证书到期时间、默认证书映射、OCSP 是否配置
- access log 已新增 `tls_version` / `tls_alpn`，默认结构化访问日志也会输出这两个字段
- reload 失败信息与 `validate_config_transition` 的 restart-boundary 字段列表已统一，避免文案和真实行为漂移

当前仍明确未支持：

- 自动 OCSP 装载状态探测与动态刷新
- 证书摘要 / 指纹视图
- 更细粒度的证书链冲突分析

目标：

- 让 TLS 运行时不再是黑盒

工作内容：

- 明确哪些 TLS 改动可以 `reload`，哪些必须 `restart`
- 在 admin `status` / `snapshot` 中增加 TLS 视图：listener TLS、默认证书、SNI 映射、证书到期时间、OCSP 装载状态
- 在 access log / runtime log 中增加 negotiated protocol、ALPN、client cert 关键信息
- 在 `rginx check` 中增加证书即将过期、OCSP 缺失、policy 冲突等诊断

验收标准：

- 线上 SSL 问题不需要抓包也能完成第一轮定位

## Phase 7: 测试矩阵、示例与迁移工具

状态：

- 已完成

本阶段已落地：

- TLS 相关集成测试矩阵已覆盖：下游 TLS policy、下游 mTLS、上游 mTLS、上游 HTTP/2、上游 SNI、access log、admin、check、reload、migrate-nginx
- `example/`、`configs/`、`README` 已补上最终 TLS 示例入口和 release gate
- `migrate-nginx` 现在能识别常见 SSL 指令：`listen ... ssl`、`ssl_certificate`、`ssl_certificate_key`、`ssl_protocols`、`ssl_client_certificate`、`ssl_verify_client`、`proxy_ssl_verify`、`proxy_ssl_trusted_certificate`、`proxy_ssl_protocols`、`proxy_ssl_certificate`、`proxy_ssl_certificate_key`、`proxy_ssl_name`、`proxy_ssl_server_name`
- 对可安全映射的上游 TLS 指令会直接迁移到 RON；对不能安全表达的 server/vhost SSL policy 会保留结果并给出明确 warning
- README 已定义 TLS 子系统的最小发布门槛

目标：

- 把 TLS 从散点功能做成项目级能力

工作内容：

- 扩充集成测试矩阵：SNI、多证书、default cert、TLS versions、ALPN、mTLS、上游 TLS、reload / restart
- 更新 `example/`、`configs/`、README、TLS 路线图文档
- 增强 `migrate-nginx`，至少对常见 SSL 指令给出迁移结果或明确 warning
- 定义 TLS 子系统的发布门槛

验收标准：

- 所有新增 TLS 功能都必须同时带测试、示例、文档
- 后续回归不再频繁把 TLS 打坏

## 推荐执行顺序

第一批：

1. Phase 0
2. Phase 1
3. Phase 2

第二批：

1. Phase 3
2. Phase 4

第三批：

1. Phase 5
2. Phase 6
3. Phase 7

原因：

- 不先固定配置边界，后续 TLS 功能会持续返工
- 不先补证书和 SNI 正确性，TLS 不能算可上线
- 不补运维和测试，后续迭代风险会很高

## 范围定义

“像 nginx 对齐”建议定义成三层：

- 行为对齐
- 配置语义尽量接近
- 对 OpenSSL 专属能力明确不支持

因此本路线图的目标不是“完全复刻 nginx SSL 指令面”，而是：

- 常用生产能力尽量对齐 nginx
- 不支持项显式报错
- 行为稳定、测试充分、运维可见

## 相关代码入口

- 服务端 TLS acceptor：`crates/rginx-http/src/tls.rs`
- 证书与 SNI 选择：`crates/rginx-http/src/tls/certificates.rs`
- 上游 TLS client：`crates/rginx-http/src/proxy/clients.rs`
- 主动健康检查 TLS 路径：`crates/rginx-http/src/proxy/health.rs`
- 服务端 TLS 配置模型：`crates/rginx-config/src/model.rs`
- 服务端 TLS 编译：`crates/rginx-config/src/compile/server.rs`
- 上游 TLS 编译：`crates/rginx-config/src/compile/upstream.rs`
- TLS 校验：`crates/rginx-config/src/validate/server.rs`、`crates/rginx-config/src/validate/upstream.rs`、`crates/rginx-config/src/validate/vhost.rs`
- reload 边界校验：`crates/rginx-http/src/state/helpers.rs`
- TLS acceptor 热替换：`crates/rginx-http/src/state.rs`
- 运行时 admin 面：`crates/rginx-runtime/src/admin.rs`
