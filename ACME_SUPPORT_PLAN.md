# rginx ACME 证书签发阶段计划

## 当前状态

- 截至 `2026-04-30`，`rginx` 还没有内置类似 `certbot` 的 ACME 自动签发工具。
- 当前 TLS 模型仍然是“从现有 PEM 文件加载证书和私钥”，并在运行时支持下游 TLS、SNI、OCSP、mTLS、ALPN、热重载和优雅重启。
- 当前运行时已经具备几个可复用的基础能力：
  - TLS acceptor 构建与证书重载
  - listener 生命周期管理
  - admin/status/snapshot 观测面
  - 后台任务生命周期
  - OCSP 定时刷新

这意味着 `rginx` 更适合把 ACME 作为独立运行时子系统接入，而不是把证书管理控制权反向交给 TLS 接入层。

## 选型结论

首选 `instant-acme`。

原因：

- `instant-acme` 是更底层的 ACME 协议客户端，适合嵌入现有 runtime。
- `rginx` 已经自己掌管 TLS 证书装载、SNI、listener 生命周期、reload 和 HTTP/3；这里不需要一个“替我们接管 TLS incoming”的框架式方案。
- `instant-acme` 更容易做成：
  - 申请证书
  - 续期证书
  - 原子写回现有 `cert_path` / `key_path`
  - 成功后刷新当前 listener 的 TLS 运行态

不选 `rustls-acme` 作为主路径的原因：

- `rustls-acme` 的强项是把 ACME 和 rustls serving 直接绑在一起。
- 对简单 HTTPS 服务它很省事，但对 `rginx` 这种已经有完整 TLS/runtime orchestration 的项目，会和现有 listener/TLS/HTTP3 结构发生重叠。
- 即使用其 low-level API，也仍然会把实现重心拉向“围绕 resolver/acceptor 接管握手”，而不是“围绕现有 runtime 子系统做编排”。

其他备选：

- `tokio-rustls-acme`：仍然更偏自动化 TLS serving 封装，不适合作为本项目的主集成路线。
- `acme-lib`：生态和接口风格都不如 `instant-acme` 贴合当前项目。

## V1 目标边界

第一版只解决最值得先落地的路径：

- 单机部署
- 自动签发和自动续期
- `HTTP-01`
- 精确域名
- 证书仍然落盘为 PEM
- 续期成功后尽量直接刷新 TLS 运行态，而不是强制全量 reload

第一版明确不做：

- wildcard 证书
- `DNS-01`
- `TLS-ALPN-01`
- 多节点共享 ACME 状态
- 自动接管任意 DNS provider
- 首版就承诺 HTTP/1.1、HTTP/2、HTTP/3 三条协议路径都无 reload 热生效

## 当前代码接点

计划基于以下已有结构展开：

- `crates/rginx-http/src/tls/acceptor.rs`
  - 负责从现有 `cert_path` / `key_path` 构建 TLS acceptor 与 HTTP/3 rustls config。
- `crates/rginx-http/src/state/lifecycle/reload.rs`
  - 已有 `refresh_tls_acceptors_from_current_config()`，适合承接“证书文件变更后刷新 TCP TLS acceptor”。
- `crates/rginx-runtime/src/bootstrap/mod.rs`
  - 已有 admin/cache/health/ocsp 后台任务挂载点，适合接入 ACME runtime task。
- `crates/rginx-runtime/src/ocsp/scheduler.rs`
  - 当前 OCSP 刷新流程可作为 ACME 任务结构和状态记录的参考。
- `crates/rginx-http/src/handler/dispatch/mod.rs`
  - 适合在常规 route 选择前短路处理 `/.well-known/acme-challenge/*`。
- `crates/rginx-http/src/server/http3/endpoint.rs`
  - HTTP/3 endpoint 当前在绑定时固化证书配置，后续需要单独处理其证书刷新路径。

## 分阶段实施计划

### 阶段 0：锁定首版边界

目标：

- 明确第一版只做 `instant-acme + HTTP-01 + 单机 + PEM 落盘`。
- 明确首版只支持 vhost 级托管证书，不直接从 listener 级默认证书起步。

主要设计：

- 不把 ACME V1 做成“自动替换所有 TLS 配置来源”的大重构。
- 首版优先支持 `servers[].tls` 的 ACME 托管模式。
- 继续保留现有静态证书路径模式，ACME 只是新增来源，不替代静态 PEM。

验收标准：

- 计划文档、配置边界和实现范围一致。
- 不把 wildcard、DNS provider、HTTP/3 热刷新一起塞进首个里程碑。

### 阶段 1：补配置模型、校验和编译结果

目标：

- 为 ACME 增加最小但清晰的配置面。

建议配置方向：

- 在顶层 `Config` 增加 `acme` 全局配置。
- 在 `VirtualHostTlsConfig` 增加 `acme` 子块。
- 继续保留 `cert_path` / `key_path`，由 ACME 负责写入这些路径。

建议全局字段：

- `directory_url`
- `contacts`
- `state_dir`
- `renew_before_days`
- `poll_interval_secs`

建议 vhost 级字段：

- `domains`
- `challenge = Http01`

主要改动范围：

- `crates/rginx-config/src/model.rs`
- `crates/rginx-config/src/model/tls.rs`
- `crates/rginx-config/src/validate/`
- `crates/rginx-config/src/compile/`
- `crates/rginx-core/src/config/`

验收标准：

- 配置可 parse、validate、compile。
- 生成统一的 `ManagedCertificateSpec` 或等价编译产物，供 runtime 遍历。
- 能拒绝明显不合法的组合：
  - wildcard + `HTTP-01`
  - `additional_certificates` + ACME
  - 未声明可写证书路径
  - 没有 HTTP listener 却启用 `HTTP-01`

### 阶段 2：落地 ACME 核心模块

目标：

- 先把 `instant-acme` 协议流程封装成可测试的内部模块。

建议模块布局：

- `crates/rginx-runtime/src/acme/mod.rs`
- `crates/rginx-runtime/src/acme/types.rs`
- `crates/rginx-runtime/src/acme/account.rs`
- `crates/rginx-runtime/src/acme/order.rs`
- `crates/rginx-runtime/src/acme/storage.rs`
- `crates/rginx-runtime/src/acme/tests.rs`

职责：

- 账户创建与恢复
- 新订单创建
- challenge 选择
- challenge ready 提交
- order ready 轮询
- finalize / poll_certificate
- 原子写证书和私钥

持久化策略：

- ACME account credentials 单独持久化到 `state_dir`
- 证书和私钥写回 `cert_path` / `key_path`
- 写文件必须原子替换，避免运行时读到半写状态

验收标准：

- 可以对单个 `ManagedCertificateSpec` 跑完整下单流程。
- 账户恢复和重复续期可复用已持久化 credentials。
- 文件写入具备 crash-safe 的最小原子性。

### 阶段 3：先解决首签冷启动

目标：

- 解决“证书文件还不存在时，主 TLS listener 无法直接启动”的问题。

现状约束：

- 当前 TLS acceptor 构建依赖现有 PEM。
- 当前 HTTP/3 endpoint 绑定也依赖现有证书。

首版策略：

- 先增加一次性签发入口，例如 `rginx acme issue --once`。
- 该入口只临时服务 `/.well-known/acme-challenge/*` 所需的 HTTP 明文验证流量。
- 签发完成后把 PEM 写到配置路径，再正常启动 `rginx`。

原因：

- 这条路径最小化对现有 listener/bootstrap 的侵入。
- 可以先把“证书签下来”这件事独立做成可验收能力，再接自动续期。

验收标准：

- 在空证书目录场景下，可以通过一次性命令完成初始签发。
- 正常启动路径仍然维持现有 TLS 架构，不要求首轮即支持 certless HTTPS 冷启动。

### 阶段 4：把自动续期接入 runtime 后台任务

目标：

- 让 ACME 像现有 OCSP 一样成为长期运行的后台任务。

建议接点：

- 在 `crates/rginx-runtime/src/bootstrap/mod.rs` 新增 `acme_task`
- 任务签名风格对齐当前 `ocsp::run(state.http.clone(), shutdown_tx.subscribe())`

任务职责：

- 订阅 config revision 变化
- 定期扫描托管证书 spec
- 判断是否需要申请/续期：
  - 证书缺失
  - SAN 不匹配
  - 临近过期
- 限制并发
- 做错误退避

验收标准：

- 启动后可后台扫描托管证书。
- config reload 后能重新 reconcile 证书任务集合。
- 失败不会破坏已有可用证书文件。

### 阶段 5：接入 HTTP-01 challenge 响应与 TCP TLS 热刷新

目标：

- 让正常运行中的 `rginx` 可以完成 `HTTP-01` 验证，并在续期成功后刷新 TCP TLS acceptor。

实现方向：

- 在 `crates/rginx-http/src/handler/dispatch/mod.rs` 最前面短路：
  - `/.well-known/acme-challenge/*`
- challenge token 存放在共享运行时状态中
- ACME task 在 `set_ready()` 前注册 token，在完成后清理

运行时刷新：

- 成功写入新 PEM 后调用
  `refresh_tls_acceptors_from_current_config()`
- 对 TCP TLS listener，这样可以做到不触发全量 reload 的证书切换

配套注意点：

- challenge 路径必须绕过普通 route、鉴权、限流和 HTTP 到 HTTPS 重定向
- 只允许返回当前 challenge token 对应内容

验收标准：

- `HTTP-01` 验证请求可由正式运行中的 `rginx` 返回正确响应。
- 新证书落盘后，HTTP/1.1 和 HTTP/2 下游握手可以切到新证书。
- 不要求此阶段同步解决 HTTP/3 证书热刷新。

### 阶段 6：补 HTTP/3、OCSP 和协议差异收口

目标：

- 把“证书续期成功”从 TCP-only 收口到完整 TLS 运行时语义。

当前关键约束：

- TCP listener 的 TLS acceptor 已有独立刷新接口。
- HTTP/3 endpoint 当前在绑定时固化 `quinn::ServerConfig`，不能直接复用 TCP acceptor 刷新路径。

实现方向：

- 短期策略二选一：
  - V1 采用：限制 ACME 托管 listener 暂不启用 `http3`
  - 或者补一条 HTTP/3 endpoint 的 server config 更新路径
- ACME 成功后补一次 OCSP refresh，避免新证书刚切换时 staple 缺失

验收标准：

- 文档和实现对 HTTP/3 行为保持一致，不制造“TCP 已热刷新但 HTTP/3 仍旧证书”的隐藏语义。
- 新证书切换后，OCSP 状态能够重新建立。

### 阶段 7：补状态观测、CLI 和硬化

目标：

- 让 ACME 子系统具备可观测性和运维可诊断性。

建议状态字段：

- `scope`
- `domains`
- `managed`
- `last_success_unix_ms`
- `next_renewal_unix_ms`
- `refreshes_total`
- `failures_total`
- `last_error`
- `challenge_type`
- `directory_url`

建议暴露面：

- `status`
- `snapshot`
- `check`
- 未来可选的 `rginx acme status`

还需补的硬化项：

- challenge 并发控制
- 目录锁或文件锁
- ACME 服务端退避与限速
- 更严格的证书路径权限检查
- staging / production directory 明确区分

验收标准：

- 运维可以从现有状态面看见 ACME 健康度。
- 失败时能定位到证书 scope、最近错误和重试状态。
- staging 验证流程和 production 上线流程可以分开执行。

## 建议 PR 切分

建议按 4 轮拆分，避免单次改动过大：

1. `schema + validate + compile + doc`
2. `acme issue --once`
3. `runtime renewer + http-01 responder + tcp tls refresh`
4. `http3 + ocsp + status + hardening`

## V1 完成定义

当以下条件全部满足时，可认为 ACME V1 完成：

- `rginx` 具备内置 ACME 证书签发能力
- 单机 `HTTP-01` 初始签发可用
- 单机 `HTTP-01` 自动续期可用
- 新证书可写回现有 `cert_path` / `key_path`
- TCP TLS listener 可在不做全量 reload 的情况下切换到新证书
- ACME 状态可通过现有运维面查看
- 文档明确说明 HTTP/3 的当前行为和限制

## 后续扩展方向

V1 之后再考虑：

- `DNS-01`
- wildcard 证书
- 多账户/多 directory
- 多节点共享 ACME 状态
- 真正的 certless HTTPS 冷启动
- HTTP/3 证书热刷新彻底收口
