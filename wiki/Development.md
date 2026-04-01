# Development

本页面向准备继续扩展 `rginx` 的开发者。

如果你还没读过项目结构，建议先看：

- [Architecture](Architecture.md)

## 先建立的几个心智模型

改 `rginx` 时，建议先记住下面这几个原则：

1. 配置不是直接在请求路径里解释执行，而是先编译成 `ConfigSnapshot`。
2. 请求主路径统一走 `handler/dispatch.rs`，不要在多个入口里平行塞逻辑。
3. proxy 行为优先落在 `proxy/` 下对应子模块，而不是回到一个“大而全”的文件。
4. 热重载和动态配置更新，本质上都是“生成新快照并替换 active state”。
5. 新能力默认要同时补测试和文档。

## 代码落点速查

下面这张表是最常用的“改动落点”索引：

| 需求类型 | 主要文件 |
| --- | --- |
| 新增原始配置字段 | `crates/rginx-config/src/model.rs` |
| 配置预处理 | `crates/rginx-config/src/load.rs` |
| 配置语义校验 | `crates/rginx-config/src/validate.rs` |
| 配置编译总入口 | `crates/rginx-config/src/compile.rs` |
| runtime 编译 | `crates/rginx-config/src/compile/runtime.rs` |
| server 默认值、路径归一化与 TLS / trusted proxy 编译 | `crates/rginx-config/src/compile/server.rs` |
| upstream transport / health 编译 | `crates/rginx-config/src/compile/upstream.rs` |
| route action、ACL 与限流编译 | `crates/rginx-config/src/compile/route.rs` |
| vhost 编译 | `crates/rginx-config/src/compile/vhost.rs` |
| 共享运行时模型 | `crates/rginx-core/src/config.rs` |
| 请求主调度 | `crates/rginx-http/src/handler/dispatch.rs` |
| access log | `crates/rginx-http/src/handler/access_log.rs` |
| 管理接口（`/status`、`/metrics`、`Config`） | `crates/rginx-http/src/handler/admin.rs` |
| gRPC / grpc-web 观测与错误映射 | `crates/rginx-http/src/handler/grpc.rs` |
| vhost / route 选择 | `crates/rginx-http/src/router.rs` |
| 代理 header / URI / 通用 helper | `crates/rginx-http/src/proxy/mod.rs` |
| upstream client 缓存与 TLS profile | `crates/rginx-http/src/proxy/clients.rs` |
| 请求体预处理与重放判断 | `crates/rginx-http/src/proxy/request_body.rs` |
| 主转发流程 | `crates/rginx-http/src/proxy/forward.rs` |
| 主动健康检查编排与 health request 构造 | `crates/rginx-http/src/proxy/health.rs` |
| peer 健康状态、least_conn 与 active request 计数 | `crates/rginx-http/src/proxy/health/registry.rs` |
| gRPC health probe 编解码与结果判定 | `crates/rginx-http/src/proxy/health/grpc_health_codec.rs` |
| grpc-web 编解码 | `crates/rginx-http/src/proxy/grpc_web.rs` |
| Upgrade / WebSocket 隧道 | `crates/rginx-http/src/proxy/upgrade.rs` |
| 静态文件 | `crates/rginx-http/src/file.rs` |
| 响应压缩 | `crates/rginx-http/src/compression.rs` |
| 客户端 IP 解析 | `crates/rginx-http/src/client_ip.rs` |
| 限流 | `crates/rginx-http/src/rate_limit.rs` |
| 连接层与协议切换 | `crates/rginx-http/src/server.rs` |
| TLS acceptor 与 SNI | `crates/rginx-http/src/tls.rs` |
| 共享状态与热替换 | `crates/rginx-http/src/state.rs` |
| 运行时编排 | `crates/rginx-runtime/src/bootstrap.rs` |
| reload | `crates/rginx-runtime/src/reload.rs` |
| 主动健康检查调度 | `crates/rginx-runtime/src/health.rs` |
| logging 初始化 | `crates/rginx-observability/src/logging.rs` |

## 当前目录边界

当前最重要的结构变化，是 HTTP 层已经不是旧的单文件模式，而是目录化后的形态：

```text
crates/rginx-http/src/handler/
  mod.rs
  dispatch.rs
  admin.rs
  grpc.rs
  access_log.rs

crates/rginx-http/src/proxy/
  mod.rs
  clients.rs
  request_body.rs
  forward.rs
  health.rs
  health/
    registry.rs
    grpc_health_codec.rs
  grpc_web.rs
  upgrade.rs
```

因此，如果你在补文档、写评审意见或设计新功能时，还在说“改 `handler.rs` / `proxy.rs`”，那通常已经是旧说法。

## 常用命令

格式化：

```bash
cargo fmt --all
```

全工作区回归：

```bash
cargo test --workspace
```

仅跑默认包：

```bash
cargo test -p rginx
```

只检查配置链路：

```bash
cargo run -p rginx -- check --config configs/rginx.ron
```

启动本地实例：

```bash
cargo run -p rginx -- --config configs/rginx.ron
```

更详细日志：

```bash
RUST_LOG=debug,rginx_http=trace cargo run -p rginx -- --config configs/rginx.ron
```

## 功能开发的推荐顺序

如果你要新增一个能力，建议按下面顺序推进：

1. 在 `model.rs` 增加原始配置字段。
2. 在 `validate.rs` 增加约束。
3. 在 `compile.rs` 做默认值、编译和运行时映射。
4. 在 `rginx-core` 扩展运行时结构。
5. 在 `rginx-http` 或 `rginx-runtime` 中落真正行为。
6. 先补单元测试，再补集成测试。
7. 更新 README、ROADMAP 和对应 wiki 页面。

这套顺序的价值在于：

- 避免请求路径里临时“偷解析”原始配置。
- 避免只改行为、不补编译和约束，导致配置面失真。
- 避免 feature 落地后文档继续讲旧状态。

## 新增能力时的判断规则

### 什么时候改 `rginx-config`

当能力涉及下列任意一项时，应优先从 `rginx-config` 开始：

- 新配置字段
- 新的跨字段约束
- 新默认值
- 需要把字符串 / path / timeout / header / CIDR 编译成更稳定的运行时结构

### 什么时候改 `rginx-core`

当能力需要变成“请求路径直接消费的稳定模型”时，应在 `rginx-core` 增加或调整结构：

- 新 route action
- 新 upstream setting
- 新 access log 字段
- 新 route / vhost 匹配条件

### 什么时候改 `rginx-http`

绝大多数用户可见行为都落在这里：

- 请求路由
- 代理行为
- 文件服务
- metrics / status
- ACL / 限流
- TLS / HTTP/2 / grpc-web

### 什么时候改 `rginx-runtime`

只有当能力属于“进程级生命周期”时，才应优先落在 `rginx-runtime`：

- 信号处理
- 热重载
- 后台任务调度
- graceful shutdown

## 测试结构

当前测试大致分三层：

### 单元测试

分布在各 crate 的 `src/*.rs` 和子模块里。

典型覆盖：

- route 匹配
- 配置校验
- 配置编译
- upstream 选择
- health / failover 逻辑
- metrics
- compression
- TLS 构造
- grpc-web 编解码

### 集成测试

位于：

- `crates/rginx-app/tests/`

典型覆盖：

- 配置检查
- vhost
- HTTP/2
- gRPC / grpc-web
- failover / backup / `ip_hash` / `least_conn`
- compression
- reload
- upgrade
- hardening
- 动态配置 API

### 共享测试 harness

最近集成测试层已经统一出共享 harness：

- `crates/rginx-app/tests/support/mod.rs`

它负责：

- 启动 `rginx` 子进程
- 写测试配置
- 收集 stdout / stderr
- HTTP / HTTPS ready probe
- 统一退出和超时失败信息

如果你要继续增加新的集成测试，优先复用这层 harness，而不是在新测试文件里再复制一套 child process 管理逻辑。

## 调试建议

### 请求路径异常时

优先按下面顺序查：

1. 是否选中了正确 vhost。
2. 是否选中了正确 route。
3. ACL 或限流是否先拦截了请求。
4. handler 是否走到了预期 action。
5. proxy 侧选中了哪个 peer。
6. peer 是否因为 passive / active health 被摘除。
7. `trusted_proxies` 是否导致客户端 IP 被错误识别。
8. gRPC / grpc-web 是否因为 content-type、trailers 或 timeout 进入特殊分支。

### reload 异常时

优先查：

1. 新配置是否能单独通过 `rginx check`。
2. 是否修改了 `listen`、`runtime.worker_threads` 或 `runtime.accept_workers`。
3. 动态配置 API 写回的内容是否就是你预期的文档。
4. 当前 active revision 是否真的变化了。

### 集成测试偶发失败时

优先查：

1. 是否复用了 shared harness。
2. 是否只检查了“端口能连”，而没有检查真正就绪。
3. 失败信息里是否包含 child stdout / stderr。
4. 测试是否错误假设了 h2 / TLS / grpc-web 的握手顺序。

## 文档更新要求

当前仓库里，文档已经不是可选附件，而是功能的一部分。

新增或调整能力时，建议至少核对下面几项：

1. `README.md`
2. `ROADMAP.md`
3. 对应 wiki 页面
4. `Release-Gate.md` 是否需要调整稳定承诺

如果代码已经改了，但 README / ROADMAP / wiki 仍然是旧边界，后续维护成本会迅速失控。

## 当前最值得继续整理的区域

虽然 `handler/` 和 `proxy/` 已经拆得比较自然，但下面这些文件仍值得继续观察：

- `crates/rginx-core/src/config.rs`
- `crates/rginx-config/src/validate.rs`
- `crates/rginx-http/src/proxy/health.rs`

继续拆这些文件时，建议遵循两个原则：

- 只按子领域拆，不按行数机械切文件。
- 拆分前先确定测试和文档也会同步跟上。

## 推荐阅读

- [Architecture](Architecture.md)
- [Configuration](Configuration.md)
- [Roadmap and Gaps](Roadmap-and-Gaps.md)
- [Refactor Plan](Refactor-Plan.md)
- [Release Gate](Release-Gate.md)
