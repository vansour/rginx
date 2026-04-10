# Runtime Architecture

## 总览

当前运行时可以按四层理解：

1. `SharedState` facade
2. proxy orchestration
3. transition planner
4. listener supervisor

它们分别解决不同问题，不应该重新耦合回去。

## SharedState Facade

入口在：

- `crates/rginx-http/src/state.rs`

实现拆分在：

- `crates/rginx-http/src/state/connections.rs`
- `crates/rginx-http/src/state/snapshot_bus.rs`
- `crates/rginx-http/src/state/traffic.rs`
- `crates/rginx-http/src/state/upstreams.rs`
- `crates/rginx-http/src/state/lifecycle.rs`
- `crates/rginx-http/src/state/tls_runtime.rs`

职责：

- 持有当前活跃配置和 proxy client 集合
- 暴露流量统计、upstream 统计、snapshot / delta、reload 历史、TLS 运行时快照
- 管理 listener 连接计数、retired listener 运行时句柄和后台任务

约束：

- `SharedState` 是 facade，不应继续演化成“大文件 + 全部逻辑直写”
- 新能力应优先放进对应子模块，再由 facade 统一导出

## Proxy Orchestration

入口在：

- `crates/rginx-http/src/proxy/mod.rs`

关键实现：

- `forward.rs`
- `request_body.rs`
- `clients.rs`
- `health.rs`
- `grpc_web.rs`
- `common.rs`
- `error_mapping.rs`

职责：

- 下游请求预处理
- peer 选择
- failover 边界控制
- gRPC / grpc-web 协议转换
- 上游 TLS client 复用和健康状态感知

约束：

- `mod.rs` 只应保留装配与导出
- 协议细节和错误映射不应重新堆回入口文件

## Transition Planner

入口在：

- `crates/rginx-http/src/transition.rs`

职责：

- 定义当前 reloadable / restart-required 边界
- 统一输出配置过渡计划
- 为 reload 校验、`check` 输出和 TLS reload boundary 提供单一来源

约束：

- restart boundary 的知识必须集中在这里
- 新增可热更新字段时，优先更新 planner，再让调用方复用

## Listener Supervisor

入口在：

- `crates/rginx-runtime/src/bootstrap.rs`

职责：

- 维护 active listener groups
- 维护 draining listener groups
- 在 reload 时先准备新增 listener 的绑定，再切换配置，再回收被移除 listener
- 在 shutdown / abort 路径中统一管理 listener worker 生命周期

当前语义：

- 新增 listener：先 bind，再在新配置生效后启动 accept workers
- 删除 listener：停止 accept，新请求不再进入，已有连接继续 drain
- 既有 listener `listen` 地址变化：仍然交给 restart boundary

约束：

- listener 生命周期应留在 supervisor，不应再分散回 runtime 主循环和各类临时 `Vec<JoinHandle<_>>`
- retired listener 的连接 drain 依赖 `SharedState` 中的 retired runtime 句柄，不应跳过这一层

## 推荐阅读顺序

如果要理解当前运行时，建议按这个顺序读源码：

1. `crates/rginx-runtime/src/bootstrap.rs`
2. `crates/rginx-http/src/transition.rs`
3. `crates/rginx-http/src/state.rs`
4. `crates/rginx-http/src/state/*.rs`
5. `crates/rginx-http/src/proxy/mod.rs`
6. `crates/rginx-http/src/proxy/*.rs`
