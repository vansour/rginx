# rginx 修护与重构计划

## 背景

当前代码库整体分层是清楚的，但有三块已经开始成为后续演进的主要阻力：

1. `crates/rginx-http/src/state.rs` 体量过大，承担了共享状态、流量统计、TLS 运行时快照、reload 历史、后台任务管理、snapshot/delta 生成等多类职责。
2. `crates/rginx-http/src/proxy/mod.rs` 以及周边测试仍然偏重，gRPC、grpc-web、failover、header sanitize、active health 相关知识较集中，理解和修改成本高。
3. 热重载边界目前是硬编码策略。它在现阶段是合理的，但如果后续要支持 listener 动态增删，就会牵动 `bootstrap`、`state`、`restart` 等多个模块。

这个计划的目标不是立刻引入新能力，而是分阶段降低维护成本，并为后续运行时演进预留空间。

## 总体原则

- 先稳行为，再动结构。
- 先做低风险的可维护性治理，再做运行时拓扑演进。
- 每一阶段都要有明确的“不改什么”边界，避免在同一轮里同时引入语义变化和结构变化。
- 所有重构都必须由现有集成测试和新增表征测试兜底。

## 非目标

- 这一计划不要求重写配置 DSL。
- 这一计划不要求引入插件系统或远程控制面。
- 这一计划不要求在早期阶段改变现有 admin 输出格式。
- 这一计划不要求在早期阶段放宽当前的 reload/restart 边界。

## Phase 0: 行为冻结与回归防线

### 目标

先把现有行为锁住，给后续文件拆分和内部重组提供安全边界。

### 工作内容

- 补一组围绕高风险区域的表征测试：
  - `snapshot / snapshot-version / delta / wait`
  - reload 成功 / 失败 / rollback 语义
  - gRPC / grpc-web 错误映射
  - failover 与重试边界
  - TLS 证书、OCSP、mTLS、SNI 相关状态输出
- 将 CI 或本地脚本里的测试入口至少分成两层：
  - 快速层：`rginx-core`、`rginx-config`、`rginx-http` 单测
  - 慢速层：`reload`、`admin`、`grpc_proxy`、`downstream_mtls` 等集成测
- 记录当前关键行为作为重构基线，避免“结构重构顺手改行为”。

### 不改动

- 不改模块边界。
- 不改对外配置语义。
- 不改 admin schema。

### 完成标准

- 当前 workspace 测试保持全绿。
- 新增一组专门覆盖重构风险点的测试。
- 提供独立的快测与慢测入口脚本，并让 CI 按分层执行。

## Phase 1: 拆解 `state.rs`

### 目标

把当前集中在 `crates/rginx-http/src/state.rs` 的职责拆成稳定、可独立演进的状态子模块。

### 工作内容

- 保留 `SharedState` 作为对外 facade。
- 将内部状态和实现按领域拆分为独立文件，建议至少包括：
  - `state/runtime_store.rs`
  - `state/snapshot_bus.rs`
  - `state/connections.rs`
  - `state/traffic.rs`
  - `state/upstreams.rs`
  - `state/tls_runtime.rs`
  - `state/reload_history.rs`
  - `state/background.rs`
- 将当前 `record_*` 方法按领域下沉：
  - 下游连接与请求计数归 `traffic` / `connections`
  - 上游请求与 peer 结果归 `upstreams`
  - TLS / OCSP 运行时快照归 `tls_runtime`
  - reload 历史归 `reload_history`
- 让 `snapshot` 和 `delta` 依赖抽象后的组件接口，而不是直接依赖一个超大结构体的所有字段。

### 不改动

- 不改 `SharedState` 对外方法签名，除非只是内部可见接口。
- 不改 snapshot 输出内容。
- 不改 reload 判定逻辑。

### 完成标准

- `state.rs` 变成 facade 和模块装配层，而不是主实现承载层。
- 领域状态更新逻辑不再横跨多个无关职责。

## Phase 2: 继续瘦身 `proxy` 层

### 目标

让 `crates/rginx-http/src/proxy/mod.rs` 回到模块入口职责，降低协议知识集中度。

### 工作内容

- 把仍留在 `proxy/mod.rs` 的策略函数继续下沉，建议拆出：
  - `proxy/error_mapping.rs`
  - `proxy/header_sanitize.rs`
  - `proxy/grpc_timeout.rs`
  - `proxy/proxy_uri.rs`
- 维持当前已存在的子模块边界：
  - `forward.rs`
  - `clients.rs`
  - `request_body.rs`
  - `health.rs`
  - `grpc_web.rs`
  - `upgrade.rs`
- 将 `mod.rs` 里的单元测试迁移到各自职责文件，减少“所有代理逻辑都从一个文件进出”的认知负担。
- 保持 `forward_request` 的生命周期清晰：
  - 请求预处理
  - peer 选择
  - 超时与失败分类
  - 可回放请求的 failover
  - 上游响应改写与下游返回

### 不改动

- 不改 failover 语义。
- 不改 gRPC / grpc-web 协议处理语义。
- 不改 upstream client profile 的缓存键规则。

### 完成标准

- `proxy/mod.rs` 只承担 re-export、共享常量和模块装配。
- 代理层的协议逻辑可按文件独立理解。

## Phase 3: 收口热重载边界为单一来源

### 目标

把当前分散在多个位置的 reload/restart 判定知识收敛为一套统一规则。

### 工作内容

- 引入统一的过渡规划器，例如：
  - `ConfigTransitionPlan`
  - `TransitionPlanner`
- 输入：旧 `ConfigSnapshot` 与新 `ConfigSnapshot`
- 输出：明确的过渡类型和原因，例如：
  - `HotReload`
  - `TlsRefreshOnly`
  - `RestartRequired`
- 将以下逻辑统一到同一来源：
  - reload 成功 / 失败判定
  - `check` 命令里的 restart boundary 输出
  - admin / status 中的 reload 相关说明
  - TLS 相关字段的 reloadable / restart-required 描述

### 不改动

- 先不改变现有规则本身。
- 先不放开任何当前需要 restart 的字段。

### 完成标准

- reload 和 restart 边界的知识不再分散。
- 文案、行为、状态输出来源一致。

## Phase 4: 引入 listener 级生命周期管理

### 目标

如果后续确认需要支持 listener 动态增删，再进入这一阶段，把一次性启动模型升级为可增量管理的 supervisor 模型。

### 工作内容

- 在 `crates/rginx-runtime` 中引入 `ListenerSupervisor` 或同等抽象。
- 每个 listener 具备独立生命周期对象，至少包含：
  - bind handle
  - accept worker group
  - shutdown channel
  - listener 级共享配置
  - TLS acceptor 引用
- reload 时先做 listener diff：
  - unchanged: 复用
  - added: 新建 bind 和 accept workers
  - removed: 停止 accept 并等待 drain
  - mutable fields changed: 原位替换共享配置或 acceptor
- 保持全局 worker 拓扑不变，`worker_threads` 与 `accept_workers` 仍然属于 restart boundary。

### 不改动

- 不在这一阶段同步放开 `worker_threads` 或 `accept_workers` 的热更新。
- 不引入远程控制面。

### 完成标准

- 在不改变全局 worker 拓扑的前提下，支持 listener 动态增删。
- listener 级别的生命周期和进程级生命周期解耦。

## Phase 5: 收尾、回归与文档化

### 目标

把前面几阶段形成的新边界稳定下来，避免重构完成后知识重新散落。

### 工作内容

- 增补以下测试：
  - listener diff 与动态增删
  - listener 下线期间的连接 drain
  - reload rollback 与 admin 状态联动
  - snapshot / delta 对新生命周期模型的覆盖
- 更新 README 和默认配置说明，明确：
  - 哪些字段可以热更新
  - 哪些字段仍需 restart
  - listener 动态增删的能力边界
- 为内部维护者补充架构文档，至少覆盖：
  - `SharedState` facade
  - proxy orchestration
  - transition planner
  - listener supervisor

### 完成标准

- 新架构有稳定测试覆盖。
- 对外与对内文档都能解释当前 reload/restart 语义。

## 优先级建议

建议按以下顺序推进：

1. `Phase 0`
2. `Phase 1`
3. `Phase 2`
4. `Phase 3`
5. 只有在明确需要 listener 动态增删时，才进入 `Phase 4` 和 `Phase 5`

原因很直接：

- `Phase 0` 到 `Phase 2` 可以明显降低维护成本，但几乎不改变产品能力边界。
- `Phase 3` 可以把 reload 规则从“写在多个地方的知识”变成单一来源。
- `Phase 4` 才是真正改变运行时拓扑的阶段，风险最高，不应该过早开始。

## 预期收益

- 降低 `state.rs` 和 `proxy/mod.rs` 的理解门槛。
- 缩小协议处理、状态聚合、运行时编排之间的耦合面。
- 让 reload/restart 边界变得可解释、可测试、可扩展。
- 为未来支持 listener 动态增删预留足够清晰的演进路径。
