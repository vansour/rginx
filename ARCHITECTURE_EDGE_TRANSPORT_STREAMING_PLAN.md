# rginx 无缓存边缘传输专项计划

## 背景

当前仓库已经具备较完整的 HTTP 入口代理能力：

- HTTP / HTTPS / HTTP/2 / HTTP/3 下游接入
- HTTP/1.1 / HTTP/2 / HTTP/3 上游代理
- gRPC / grpc-web 入口转换
- TLS / SNI / OCSP / mTLS
- 路由、限流、ACL、健康检查、优雅重启、本地运维面

但如果当前阶段明确不做缓存，那么 `rginx` 的下一阶段定位不应再是“CDN cache node”，而应收口为：

- 面向中小规模部署的 HTTP 边缘传输代理
- 关注大请求、大响应、长连接、流式传输、源站保护和可观测性
- 不追求成为完整 nginx DSL 或静态文件服务器

## 当前基线

从当前实现看，项目已经非常接近“边缘传输代理”，但还存在几处与该目标不完全一致的行为：

- 请求体限额与请求体收集存在耦合
  - 当启用 `server.max_request_body_bytes` 时，请求体会进入整体收集路径，而不是纯流式限额。
- failover 语义依赖请求体可重放
  - 当前这是合理的，但需要显式和 buffering policy 打通。
- 响应压缩为 collect-then-compress
  - 当前实现适合普通 API，不适合大对象和长时间响应。
- 长连接测试覆盖存在，但还不够聚焦“慢连接 / 大流量 / reload / restart”专项。

## 目标

本专项只解决传输层问题，不引入缓存。

目标如下：

- 完善流式请求 / 响应传输路径
- 明确请求缓冲、响应缓冲、压缩策略的配置与默认行为
- 保持 failover、超时、压缩、upgrade、gRPC、HTTP/3 之间的语义一致性
- 建立覆盖 HTTP/1.1、HTTP/2、HTTP/3 的长连接专项测试矩阵
- 为后续源站保护、连接治理和链路观测打稳定基础

## 非目标

本专项明确不包含：

- 任何共享缓存、对象缓存、revalidate、purge、origin shield
- 完整 rewrite DSL
- 静态文件服务
- `stream` / `mail` / FastCGI / uwsgi 入口代理
- 大规模控制面重构

## 设计原则

### 1. 传输优先于功能

优先保证“请求 / 响应如何流动”是正确的，再考虑附加能力。

### 2. 默认保守

默认行为应尽量避免隐式大缓冲、隐式 body 收集、隐式语义变更。

### 3. 配置显式

需要通过配置明确说明：

- 何时允许缓冲
- 何时必须流式直通
- 何时允许为了 failover 牺牲流式能力
- 何时允许压缩

### 4. HTTP/1.1、HTTP/2、HTTP/3 语义对齐

同一条路由在不同下游协议上，默认应保持一致的传输语义和失败行为。

## Phase 0：配置面与语义边界

本阶段不改变默认运行时行为，先把配置面和边界定义清楚。

### 目标

- 引入请求缓冲、响应缓冲、压缩策略配置
- 为后续实现建立 validate / compile 入口
- 明确默认值、非法组合和降级规则

### 建议配置

- `request_buffering: Auto | On | Off`
- `response_buffering: Auto | On | Off`
- `compression: Off | Auto | Force`
- `compression_min_bytes`
- `compression_content_types`
- 可选：`streaming_response_idle_timeout`

### 建议默认语义

- `request_buffering=Auto`
  - 小且可重放的幂等请求允许进入可重放路径
  - 其余请求默认走流式路径
- `request_buffering=Off`
  - 优先保证流式传输
  - 禁止依赖完整请求体的 failover
- `request_buffering=On`
  - 允许主动收集请求体
  - 必须受明确大小限制保护
- `response_buffering=Auto`
  - 默认以流式直通为主
  - 仅在明确需要的场景进入收集路径
- `compression=Auto`
  - 仅对安全且有收益的响应启用压缩
- `compression=Off`
  - 永不压缩
- `compression=Force`
  - 只在已知安全前提下强制压缩

### 实现落点

- `crates/rginx-core/src/config/route.rs`
- `crates/rginx-config/src/model.rs`
- `crates/rginx-config/src/validate/route.rs`
- `crates/rginx-config/src/compile/route.rs`
- `crates/rginx-app/src/main.rs`

### 退出标准

- 配置模型和默认值确定
- `rginx check` 可以展示相关策略
- 非法组合有清晰报错
- 不引入运行时行为回归

## Phase 1：请求侧流式传输与缓冲策略

本阶段优先处理上传和上游请求构建。

### 目标

- 将“请求体大小限制”和“请求体是否收集为 replayable”解耦
- 在 `request_buffering=Off` 时支持真正的流式上传
- 保持当前幂等 + 可重放 failover 语义，但不再绑架所有请求进入缓冲路径

### 主要工作

- 新增 streaming size guard
  - 在不整体收集请求体的情况下执行字节上限检查
- 将请求处理分成三类
  - 可重放缓冲
  - 单次流式透传
  - 明确拒绝
- `request_buffering=Off` 时禁用基于 replay 的 peer failover
- `request_buffering=Auto` 时仅对“小体积 + 幂等 + 可安全收集”的请求启用 replayable 路径
- 明确 trailers 透传和 `TE: trailers` 的边界

### 受影响文件

- `crates/rginx-http/src/proxy/request_body.rs`
- `crates/rginx-http/src/proxy/forward/mod.rs`
- `crates/rginx-http/src/timeout/body.rs`

### 测试重点

- 大上传在 `request_buffering=Off` 下不整体收集
- 流式上传中途超限时能及时终止
- `Off` 下上游失败不触发 failover
- `Auto` 下小幂等请求仍保持现有 failover 能力
- HTTP/1.1 / HTTP/2 / HTTP/3 下行为一致

### 退出标准

- 大请求默认不再因限额策略被隐式全量收集
- failover 与 buffering policy 的关系清晰且稳定
- 现有 proxy / gRPC / HTTP/3 测试无回归

## Phase 2：响应侧流式传输与压缩策略

本阶段处理下载、长响应和动态压缩。

### 目标

- 让响应默认更接近传输直通，而不是“先收集后处理”
- 将压缩策略与响应缓冲策略解耦
- 避免压缩破坏长连接、大对象或协议语义

### 主要工作

- `response_buffering=Off` 时默认直通响应 body
- `compression=Auto` 时严格限制压缩触发条件
- 第一版不追求 streaming compression
  - 先做“安全禁压缩”
  - 后续再评估真正的流式 gzip / br
- 明确以下场景默认不压缩
  - `206 Partial Content`
  - `Content-Range`
  - 已有 `Content-Encoding`
  - `Upgrade` / WebSocket
  - gRPC 流
  - 不适合收集的大响应

### 受影响文件

- `crates/rginx-http/src/compression.rs`
- `crates/rginx-http/src/handler/dispatch.rs`
- `crates/rginx-http/src/handler/grpc.rs`

### 测试重点

- 大响应在 `response_buffering=Off` 下不整体收集
- `Auto` 模式只对安全场景压缩
- gRPC / grpc-web / upgrade 不进入错误压缩路径
- `Vary: Accept-Encoding` 继续正确维护
- HTTP/3 下长响应行为与 HTTP/1.1 / HTTP/2 保持一致

### 退出标准

- 默认响应路径不再破坏流式传输
- 压缩不会误伤协议语义
- 动态压缩从“功能优先”改为“传输安全优先”

## Phase 3：长连接专项测试与稳定性硬化

本阶段以测试和硬化为主，不以新增功能为主。

### 目标

- 建立长连接和大流量场景的专项回归集
- 明确 reload / restart / drain 与长连接之间的行为
- 为发布门禁提供可重复运行的 soak 测试入口

### 重点场景

- 慢上传
- 慢下载
- 大请求体
- 大响应体
- WebSocket 长连接
- HTTP/2 长流
- HTTP/3 长流
- 客户端半关闭
- 上游半关闭
- 传输过程中 reload
- 传输过程中 restart
- drain 与 shutdown

### 受影响文件

- `crates/rginx-runtime/src/bootstrap/listeners.rs`
- `crates/rginx-runtime/src/bootstrap/shutdown.rs`
- `crates/rginx-runtime/src/restart.rs`
- `crates/rginx-http/src/server/graceful.rs`
- `crates/rginx-app/tests/support/mod.rs`

### 建议新增测试

- `streaming_upload.rs`
- `streaming_download.rs`
- `reload_streaming.rs`
- `long_lived_http2.rs`
- `long_lived_http3.rs`

### 门禁建议

- fast gate
  - 关键语义单测
- slow gate
  - 大上传 / 大下载 / 慢连接 / reload 组合
- soak gate
  - HTTP/3 长连接、多轮 reload / restart、长时间传输稳定性

### 退出标准

- HTTP/1.1 / HTTP/2 / HTTP/3 三套协议都有专项覆盖
- reload / restart / drain 对长连接的语义可验证、可回归
- 新旧 worker 切换期间无明显截断或资源泄漏

## 推荐执行顺序

1. Phase 0：先定配置面与行为边界
2. Phase 1：只做请求侧流式与限额解耦
3. Phase 2：再做响应侧和压缩降级
4. Phase 3：最后补长连接专项测试和 soak

## 为什么按这个顺序

- 请求侧优先，是因为上传路径最容易把传输代理做成隐式缓冲代理
- 压缩放在后面，是因为它天然依赖响应传输策略
- 长连接专项测试要在行为稳定后补齐，否则会边改语义边修测试

## 阶段退出后应获得的能力

### 完成 Phase 0 后

- 项目具备明确的传输策略配置面

### 完成 Phase 1 后

- 项目可以更可靠地承担大上传和持续请求体传输

### 完成 Phase 2 后

- 项目可以更可靠地承担长响应、大响应和低缓冲响应直通

### 完成 Phase 3 后

- 项目具备面向发布的长连接稳定性回归门禁

## 后续但不在本专项内

在本专项完成后，才建议继续推进：

- 每 upstream 并发上限
- pending queue
- retry budget
- circuit breaker
- 分阶段时延观测
- 更细的连接池和连接寿命治理

这些能力建立在稳定的流式传输语义之上，应该排在本专项之后。
