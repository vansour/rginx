# 大文件拆分计划

更新时间：`2026-04-10`

## 当前阶段状态

| 阶段 | 状态 | 备注 |
| --- | --- | --- |
| 阶段 0：冻结拆分边界 | Done | 已完成目标文件盘点、优先级排序、阶段门禁与拆分顺序定义 |
| 阶段 1：基础机械拆分 | Done | 已完成 `compile` / `state` / `tls` 入口瘦身与目录化拆分 |
| 阶段 2：CLI 与迁移器拆分 | Done | 已完成 `admin_cli` / `migrate_nginx` 入口瘦身与目录化拆分 |
| 阶段 3：TLS runtime snapshot 拆分 | Done | 已完成 `tls_runtime` 入口瘦身与目录化拆分 |
| 阶段 4：runtime 生命周期拆分 | Done | 已完成 `bootstrap` / `server` / `timeout` 入口瘦身与目录化拆分 |
| 阶段 5：handler / proxy 主链路拆分 | Done | 已完成 `handler` / `proxy` 热路径入口瘦身与目录化拆分 |
| 阶段 6：大型集成测试拆分 | Done | 已完成大型测试入口按主题拆分与目录化整理 |
| 阶段 7：工具与文档拆分 | Done | 已完成 `nginx_compare` 工具包化与对比文档快照解耦 |

## 目标

这份文档的目标是给当前仓库的大文件拆分工作建立一个低风险、可分阶段落地的计划。

拆分工作的核心目标：

1. 降低单文件认知负担
2. 把“入口协调”和“具体职责实现”分离
3. 让后续功能迭代和 review 更容易收敛到局部
4. 在拆分过程中保持外部行为稳定

## 非目标

这份计划不包含以下内容：

- 不以 crate 级重组为第一目标
- 不把“拆分文件”和“重写语义”绑在一个阶段
- 不在同一个阶段同时做大规模命名迁移
- 不优先处理纯测试体量问题而忽略生产代码边界问题

## 当前体量快照

按当前仓库的粗略统计，最值得优先关注的大文件包括：

| 文件 | 约行数 | 说明 |
| --- | ---: | --- |
| [crates/rginx-config/src/compile.rs](../crates/rginx-config/src/compile/mod.rs) | 2189 | 配置编译入口与大量测试混在一起 |
| [crates/rginx-app/src/migrate_nginx.rs](../crates/rginx-app/src/migrate_nginx/mod.rs) | 1738 | tokenizer / parser / convert / render 全部堆叠 |
| [scripts/nginx_compare.py](../scripts/nginx_compare.py) | 1695 | checkout / build / launch / benchmark / render / main 混在一起 |
| [crates/rginx-http/src/proxy/tests.rs](../crates/rginx-http/src/proxy/tests/mod.rs) | 1633 | 单文件测试主题过多 |
| [crates/rginx-app/src/admin_cli.rs](../crates/rginx-app/src/admin_cli/mod.rs) | 1284 | 多个 admin 子命令打印与 socket 查询混在一起 |
| [crates/rginx-http/src/tls/ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) | 1300 | 刚稳定下来的 OCSP 子系统，短期不建议优先再拆 |
| [crates/rginx-http/src/state/tls_runtime.rs](../crates/rginx-http/src/state/tls_runtime/mod.rs) | 936 | 证书检查、OCSP 状态、binding 推导、snapshot 混在一起 |
| [crates/rginx-http/src/tls.rs](../crates/rginx-http/src/tls/mod.rs) | 845 | TLS 入口、provider 组装、session policy、SNI 逻辑集中 |
| [crates/rginx-http/src/state.rs](../crates/rginx-http/src/state/mod.rs) | 751 | 入口文件、include 模块、状态构造、内联测试并存 |
| [crates/rginx-runtime/src/bootstrap.rs](../crates/rginx-runtime/src/bootstrap/mod.rs) | 637 | runtime 生命周期协调职责较重 |

另外，测试侧的大文件也已经进入“应该拆”的区间：

- [crates/rginx-app/tests/grpc_proxy.rs](../crates/rginx-app/tests/grpc_proxy.rs)
- [crates/rginx-app/tests/admin.rs](../crates/rginx-app/tests/admin.rs)
- [crates/rginx-app/tests/reload.rs](../crates/rginx-app/tests/reload.rs)
- [crates/rginx-app/tests/downstream_mtls.rs](../crates/rginx-app/tests/downstream_mtls.rs)

## 拆分原则

### 1. 先拆边界天然清楚的文件

优先拆这类文件：

- 入口协调和实现逻辑混在一起
- 多个主题已经能按目录自然分组
- 拆分后对外 API 可以保持不变

### 2. 先拆生产代码，再拆大型测试文件

大型测试文件确实会拖慢维护，但生产代码边界不清时，测试先拆收益有限。

### 3. 每个阶段只做一种类型的拆分

不要在一个阶段里同时做：

- 目录重组
- 公共 API 调整
- 语义重写
- 命名重排

### 4. 让入口文件变薄

理想状态下，这些文件最终应该主要负责：

- 装配
- 路由
- re-export
- 子模块调用顺序

而不是承载详细实现。

### 5. 每阶段都必须有门禁

每个阶段结束至少要通过：

- `cargo check --workspace --all-targets`
- 对应该阶段影响面的定向测试
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## 当前优先级判断

### 第一优先级

- [crates/rginx-config/src/compile.rs](../crates/rginx-config/src/compile/mod.rs)
- [crates/rginx-http/src/state.rs](../crates/rginx-http/src/state/mod.rs)
- [crates/rginx-http/src/tls.rs](../crates/rginx-http/src/tls/mod.rs)
- [crates/rginx-http/src/state/tls_runtime.rs](../crates/rginx-http/src/state/tls_runtime/mod.rs)

理由：

- 拆分收益高
- 目录边界已经存在
- 对外 API 可以基本保持不变
- 风险明显低于直接拆 `proxy` / `handler` 主链路

### 第二优先级

- [crates/rginx-app/src/admin_cli.rs](../crates/rginx-app/src/admin_cli/mod.rs)
- [crates/rginx-app/src/migrate_nginx.rs](../crates/rginx-app/src/migrate_nginx/mod.rs)
- [crates/rginx-runtime/src/bootstrap.rs](../crates/rginx-runtime/src/bootstrap/mod.rs)

理由：

- 职责多，但主题边界已经相对清楚
- 主要是“转换/打印/协调”型复杂度
- 适合做结构整理而不是语义重写

### 第三优先级

- [crates/rginx-http/src/proxy/mod.rs](../crates/rginx-http/src/proxy/mod.rs)
- [crates/rginx-http/src/handler/mod.rs](../crates/rginx-http/src/handler/mod.rs)
- [crates/rginx-http/src/proxy/forward.rs](../crates/rginx-http/src/proxy/forward/mod.rs)
- [crates/rginx-http/src/proxy/clients.rs](../crates/rginx-http/src/proxy/clients/mod.rs)

理由：

- 热路径
- 协议细节多
- 拆分时最容易引入行为变化

应该放到前几轮边界整理之后再做。

### 暂缓优先级

- [crates/rginx-http/src/tls/ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)

理由：

- 刚完成依赖迁移、语义 hardening、nonce / responder policy 接入
- 短期内最重要的是稳定，不是继续切结构

## 分阶段计划

## 阶段 0：冻结拆分边界

### 目标

先固定拆分目标、目录建议和验收方式，避免后续“边拆边想”。

### 要做的事

- 固定优先级顺序
- 标出每个文件计划拆出的子模块
- 为每个阶段指定最小门禁

### 完成标准

- 本文档补齐并作为后续拆分基线

### 当前执行结果

已完成。

本阶段已做：

- 盘点当前最值得优先拆分的大文件
- 按生产代码 / 测试代码 / 工具脚本区分优先级
- 明确哪些文件现在不应该优先拆
- 给出每个阶段的建议目录与目标文件
- 固定每阶段的最小门禁
- 明确建议执行顺序

### 阶段结果判断

到阶段 0 为止，后续拆分工作已经有了稳定基线：

- 先拆什么
- 为什么先拆
- 拆到什么粒度
- 每阶段如何判断“拆分没有把仓库带坏”

下一步可以直接进入阶段 1，而不需要再重新讨论整体顺序。

## 阶段 1：基础机械拆分

### 目标

先处理最低风险、收益最高的一组。

### 目标文件

- [crates/rginx-config/src/compile.rs](../crates/rginx-config/src/compile/mod.rs)
- [crates/rginx-http/src/state.rs](../crates/rginx-http/src/state/mod.rs)
- [crates/rginx-http/src/tls.rs](../crates/rginx-http/src/tls/mod.rs)

### 建议拆法

`compile.rs`：

- `compile/mod.rs`
- `compile/path.rs`
- `compile/tests.rs`

`state.rs`：

- 把当前 `include!("state/*.rs")` 改成标准模块
- 主文件只保留 `SharedState` 入口、`mod` 声明、必要 re-export

`tls.rs`：

- `tls/mod.rs`
- `tls/acceptor.rs`
- `tls/provider.rs`
- `tls/session.rs`
- `tls/sni.rs`

### 完成标准

- 外部 API 保持不变
- 入口文件明显变薄
- 不引入新配置语义

### 当前执行结果

已完成。

本阶段已做：

- 将 [crates/rginx-config/src/compile.rs](../crates/rginx-config/src/compile/mod.rs) 目录化为：
  - `compile/mod.rs`
  - `compile/path.rs`
  - `compile/tests.rs`
- 将 [crates/rginx-http/src/state.rs](../crates/rginx-http/src/state/mod.rs) 目录化为：
  - `state/mod.rs`
  - `state/tests.rs`
- 将 [crates/rginx-http/src/tls.rs](../crates/rginx-http/src/tls/mod.rs) 目录化为：
  - `tls/mod.rs`
  - `tls/acceptor.rs`
  - `tls/client_auth.rs`
  - `tls/provider.rs`
  - `tls/session.rs`
  - `tls/sni.rs`
  - `tls/tests.rs`
- 保持现有对外 API 不变：
  - `compile`
  - `compile_with_base`
  - `build_tls_acceptor`
  - OCSP 相关对外函数路径不变

本阶段有意保留：

- `state/mod.rs` 中的 `include!("snapshots.rs") / include!("counters.rs") / include!("helpers.rs")`

原因是：

- 如果在本阶段继续强推 `state` 内部完全模块化，会把工作从“机械拆分”升级成大规模可见性重写
- 这不符合阶段 1 的低风险目标

因此，`state` 的进一步模块正规化被延后到后续阶段，而阶段 1 已经先完成了：

- 文件目录化
- 测试独立化
- 入口瘦身

### 阶段结果判断

到阶段 1 为止，这三块的结构已经发生了可见改善：

- `compile` 不再把测试堆在主文件里
- `state` 主文件不再混着大段测试
- `tls` 的 provider / session / SNI / client auth helper 已经从主文件抽离

下一步适合进入阶段 2，而不是继续在阶段 1 内扩大改动面。

## 阶段 2：CLI 与迁移器拆分

### 目标

把单文件 CLI/转换器整理成“入口 + 子模块”结构。

### 目标文件

- [crates/rginx-app/src/admin_cli.rs](../crates/rginx-app/src/admin_cli/mod.rs)
- [crates/rginx-app/src/migrate_nginx.rs](../crates/rginx-app/src/migrate_nginx/mod.rs)

### 建议拆法

`admin_cli.rs`：

- `admin_cli/mod.rs`
- `admin_cli/counters.rs`
- `admin_cli/upstreams.rs`
- `admin_cli/status.rs`
- `admin_cli/snapshot.rs`
- `admin_cli/traffic.rs`
- `admin_cli/peers.rs`
- `admin_cli/socket.rs`
- `admin_cli/render.rs`

`migrate_nginx.rs`：

- `migrate_nginx/mod.rs`
- `migrate_nginx/tokenize.rs`
- `migrate_nginx/parser.rs`
- `migrate_nginx/convert.rs`
- `migrate_nginx/render.rs`
- `migrate_nginx/tests.rs`

### 完成标准

- `mod.rs` 只剩入口
- parser / convert / render 不再互相穿插

### 当前执行结果

已完成。

本阶段已做：

- 将 [crates/rginx-app/src/admin_cli.rs](../crates/rginx-app/src/admin_cli/mod.rs) 目录化为：
  - `admin_cli/mod.rs`
  - `admin_cli/socket.rs`
  - `admin_cli/render.rs`
  - `admin_cli/snapshot.rs`
  - `admin_cli/status.rs`
  - `admin_cli/counters.rs`
  - `admin_cli/traffic.rs`
  - `admin_cli/peers.rs`
  - `admin_cli/upstreams.rs`
- 将 [crates/rginx-app/src/migrate_nginx.rs](../crates/rginx-app/src/migrate_nginx/mod.rs) 目录化为：
  - `migrate_nginx/mod.rs`
  - `migrate_nginx/tokenize.rs`
  - `migrate_nginx/parser.rs`
  - `migrate_nginx/convert.rs`
  - `migrate_nginx/render.rs`
  - `migrate_nginx/tests.rs`
- 保持现有对外入口不变：
  - `run_admin_command`
  - `migrate_file`
- 将 `migrate_nginx` 的 tokenizer / parser / convert / render 彻底分层，避免继续在一个文件里互相穿插

### 阶段结果判断

到阶段 2 为止，CLI 和 nginx 迁移器都已经从“大入口文件”收敛到“薄入口 + 职责子模块”的结构：

- `admin_cli/mod.rs` 只保留命令分发
- `migrate_nginx/mod.rs` 只保留文件读取与主流程装配
- parser、convert、render 已经分离，后续可以在局部继续细化而不需要再回到单文件结构

下一步适合进入阶段 3，处理 `tls_runtime.rs` 这类仍然体量大且主题混杂的状态/TLS 运行时文件。

## 阶段 3：TLS runtime snapshot 拆分

### 目标

把 [tls_runtime.rs](../crates/rginx-http/src/state/tls_runtime/mod.rs) 中完全不同主题的逻辑拆开。

### 目标文件

- [crates/rginx-http/src/state/tls_runtime.rs](../crates/rginx-http/src/state/tls_runtime/mod.rs)

### 建议拆法

- `state/tls_runtime/mod.rs`
- `state/tls_runtime/listeners.rs`
- `state/tls_runtime/certificates.rs`
- `state/tls_runtime/ocsp.rs`
- `state/tls_runtime/bindings.rs`
- `state/tls_runtime/upstreams.rs`
- `state/tls_runtime/reload_boundary.rs`

### 完成标准

- 证书检查、OCSP 状态、binding 推导、upstream TLS 状态分离
- `mod.rs` 只做组装

### 当前执行结果

已完成。

本阶段已做：

- 将 [crates/rginx-http/src/state/tls_runtime.rs](../crates/rginx-http/src/state/tls_runtime/mod.rs) 目录化为：
  - `state/tls_runtime/mod.rs`
  - `state/tls_runtime/listeners.rs`
  - `state/tls_runtime/certificates.rs`
  - `state/tls_runtime/ocsp.rs`
  - `state/tls_runtime/bindings.rs`
  - `state/tls_runtime/upstreams.rs`
  - `state/tls_runtime/reload_boundary.rs`
- 保持现有对外入口不变：
  - `tls_runtime_snapshot_for_config`
  - `tls_ocsp_refresh_specs_for_config`
  - `tls_reloadable_fields`
  - `tls_restart_required_fields`
- 将监听器状态、证书检查、OCSP 状态/refresh spec、binding 推导、upstream TLS 状态、reload boundary 拆成独立主题模块

### 阶段结果判断

到阶段 3 为止，TLS runtime snapshot 已经从“单文件承载全部状态推导”收敛为“薄入口 + 主题模块”结构：

- `tls_runtime/mod.rs` 只保留总装配和少量对外包装
- 证书检查与链诊断不再和 OCSP / binding / upstream 状态逻辑混在一起
- 后续如果要继续细化 TLS runtime，只需要在局部模块内演进，不需要回到 900+ 行入口文件

下一步适合进入阶段 4，整理 runtime 生命周期相关的大文件。

## 阶段 4：runtime 生命周期拆分

### 目标

整理运行时生命周期文件，降低 `startup/reload/restart/shutdown` 的耦合。

### 目标文件

- [crates/rginx-runtime/src/bootstrap.rs](../crates/rginx-runtime/src/bootstrap/mod.rs)
- [crates/rginx-http/src/server.rs](../crates/rginx-http/src/server/mod.rs)
- [crates/rginx-http/src/timeout.rs](../crates/rginx-http/src/timeout/mod.rs)

### 建议拆法

`bootstrap.rs`：

- `bootstrap/mod.rs`
- `bootstrap/listeners.rs`
- `bootstrap/reload.rs`
- `bootstrap/restart.rs`
- `bootstrap/shutdown.rs`

`server.rs`：

- `server/mod.rs`
- `server/accept.rs`
- `server/connection.rs`
- `server/graceful.rs`
- `server/proxy_protocol.rs`

`timeout.rs`：

- `timeout/mod.rs`
- `timeout/body.rs`
- `timeout/io.rs`
- `timeout/timers.rs`
- `timeout/tests.rs`

### 完成标准

- 生命周期状态机读起来不再需要在单文件里跳跃

### 当前执行结果

已完成。

本阶段已做：

- 将 [crates/rginx-runtime/src/bootstrap.rs](../crates/rginx-runtime/src/bootstrap/mod.rs) 目录化为：
  - `bootstrap/mod.rs`
  - `bootstrap/listeners.rs`
  - `bootstrap/reload.rs`
  - `bootstrap/restart.rs`
  - `bootstrap/shutdown.rs`
- 将 [crates/rginx-http/src/server.rs](../crates/rginx-http/src/server/mod.rs) 目录化为：
  - `server/mod.rs`
  - `server/accept.rs`
  - `server/connection.rs`
  - `server/graceful.rs`
  - `server/proxy_protocol.rs`
  - `server/tests.rs`
- 将 [crates/rginx-http/src/timeout.rs](../crates/rginx-http/src/timeout/mod.rs) 目录化为：
  - `timeout/mod.rs`
  - `timeout/body.rs`
  - `timeout/io.rs`
  - `timeout/timers.rs`
  - `timeout/tests.rs`
- 保持现有对外入口不变：
  - `run`
  - `serve`
  - `WriteTimeoutIo`
  - `IdleTimeoutBody`
  - `GrpcDeadlineBody`
- 将 runtime 生命周期、listener worker 生命周期、单连接处理、proxy protocol 解析、body timeout 与 write timeout 逻辑拆成独立主题模块

### 阶段结果判断

到阶段 4 为止，runtime 生命周期相关的大文件已经从“入口 + 所有细节实现堆在一起”收敛为“薄入口 + 生命周期/协议子模块”结构：

- `bootstrap/mod.rs` 只保留启动、信号循环和总装配
- `server/mod.rs` 只保留模块入口，accept loop、连接处理、graceful shutdown、proxy protocol 解析已分离
- `timeout/mod.rs` 只保留导出，body wrapper、IO wrapper 和 timer helper 不再混在一个文件里

下一步适合进入阶段 5，处理 `handler` / `proxy` 这类高耦合热路径文件。

## 阶段 5：handler / proxy 主链路拆分

### 目标

处理高耦合热路径，但只在前面边界整理之后进行。

### 目标文件

- [crates/rginx-http/src/handler/mod.rs](../crates/rginx-http/src/handler/mod.rs)
- [crates/rginx-http/src/proxy/mod.rs](../crates/rginx-http/src/proxy/mod.rs)
- [crates/rginx-http/src/proxy/forward.rs](../crates/rginx-http/src/proxy/forward/mod.rs)
- [crates/rginx-http/src/proxy/clients.rs](../crates/rginx-http/src/proxy/clients/mod.rs)
- [crates/rginx-http/src/proxy/grpc_web.rs](../crates/rginx-http/src/proxy/grpc_web/mod.rs)

### 建议拆法

- 按协议职责和热路径拆
- 不按“平均行数”机械分片

优先拆出的主题：

- request context
- header handling
- retry / failover
- grpc
- grpc-web
- upstream tls client
- body forwarding

### 完成标准

- `mod.rs` 只保留顶层流程
- 热路径逻辑能按主题定位

### 当前执行结果

已完成。

本阶段已做：

- 将 [crates/rginx-http/src/handler/mod.rs](../crates/rginx-http/src/handler/mod.rs) 进一步收敛为薄入口，并将内联测试迁移到：
  - `handler/tests.rs`
- 将 [crates/rginx-http/src/proxy/forward.rs](../crates/rginx-http/src/proxy/forward/mod.rs) 目录化为：
  - `proxy/forward/mod.rs`
  - `proxy/forward/grpc.rs`
  - `proxy/forward/error.rs`
  - `proxy/forward/response.rs`
- 将 [crates/rginx-http/src/proxy/clients.rs](../crates/rginx-http/src/proxy/clients/mod.rs) 目录化为：
  - `proxy/clients/mod.rs`
  - `proxy/clients/tls.rs`
  - `proxy/clients/tests.rs`
- 将 [crates/rginx-http/src/proxy/grpc_web.rs](../crates/rginx-http/src/proxy/grpc_web/mod.rs) 目录化为：
  - `proxy/grpc_web/mod.rs`
  - `proxy/grpc_web/body.rs`
  - `proxy/grpc_web/codec.rs`
- 保持现有对外入口不变：
  - `handle`
  - `forward_request`
  - `ProxyClients`
  - `probe_upstream_peer`
- 将 grpc-web body wrapper、grpc-web codec、upstream TLS client 构造、request timeout/grpc timeout 解析、downstream response 组装从主流程入口中拆开

### 阶段结果判断

到阶段 5 为止，`handler` / `proxy` 热路径已经从“入口文件夹带大量协议与 TLS 细节”收敛为“薄入口 + 协议/TLS/响应子模块”结构：

- `handler/mod.rs` 只保留导出和少量共享 helper
- `proxy/forward/mod.rs` 只保留主请求流程，grpc 超时解析、错误响应和响应体拼装已分离
- `proxy/clients/mod.rs` 只保留 profile 缓存和 registry 协调，TLS verifier / CA / CRL / mTLS identity 组装已分离
- `proxy/grpc_web/mod.rs` 只保留类型和导出，body wrapper 与 codec 逻辑已分离

下一步适合进入阶段 6，拆分大型集成测试文件，尤其是 `proxy/tests.rs` 和 `grpc_proxy.rs` 这类按场景堆叠的测试入口。

## 阶段 6：大型集成测试拆分

### 目标

把单文件多场景测试改成“主题 + support”结构。

### 目标文件

- [crates/rginx-app/tests/grpc_proxy.rs](../crates/rginx-app/tests/grpc_proxy.rs)
- [crates/rginx-app/tests/admin.rs](../crates/rginx-app/tests/admin.rs)
- [crates/rginx-app/tests/reload.rs](../crates/rginx-app/tests/reload.rs)
- [crates/rginx-app/tests/downstream_mtls.rs](../crates/rginx-app/tests/downstream_mtls.rs)
- [crates/rginx-http/src/proxy/tests.rs](../crates/rginx-http/src/proxy/tests/mod.rs)

### 建议拆法

不要先抽公共 helper，再让主题散落；应先按主题切：

- `grpc_proxy/basic.rs`
- `grpc_proxy/grpc_web.rs`
- `grpc_proxy/timeout.rs`
- `grpc_proxy/cancellation.rs`
- `grpc_proxy/health.rs`
- `grpc_proxy/support.rs`

其他大测试文件同理。

### 完成标准

- 每个测试文件只覆盖一类行为
- `support` 模块只承载公共工装

### 当前执行结果

已完成。

本阶段已做：

- 将 [crates/rginx-app/tests/grpc_proxy.rs](../crates/rginx-app/tests/grpc_proxy.rs) 拆为：
  - `grpc_proxy/basic.rs`
  - `grpc_proxy/timeout.rs`
  - `grpc_proxy/lifecycle.rs`
- 将 [crates/rginx-app/tests/admin.rs](../crates/rginx-app/tests/admin.rs) 拆为：
  - `admin/snapshot.rs`
  - `admin/delta_wait.rs`
  - `admin/commands.rs`
- 将 [crates/rginx-app/tests/reload.rs](../crates/rginx-app/tests/reload.rs) 拆为：
  - `reload/reload_flow.rs`
  - `reload/reload_boundary.rs`
  - `reload/restart_flow.rs`
- 将 [crates/rginx-app/tests/downstream_mtls.rs](../crates/rginx-app/tests/downstream_mtls.rs) 拆为：
  - `downstream_mtls/enforcement.rs`
  - `downstream_mtls/validation.rs`
  - `downstream_mtls/observability.rs`
- 将 [crates/rginx-http/src/proxy/tests.rs](../crates/rginx-http/src/proxy/tests/mod.rs) 目录化为：
  - `proxy/tests/mod.rs`
  - `proxy/tests/request_headers.rs`
  - `proxy/tests/grpc.rs`
  - `proxy/tests/client_profiles.rs`
  - `proxy/tests/peer_selection.rs`
  - `proxy/tests/peer_recovery.rs`
- 保持现有测试入口不变：
  - `grpc_proxy`
  - `admin`
  - `reload`
  - `downstream_mtls`
  - `proxy::tests`
- 保留现有 support / helper 基础设施，只把测试场景按主题迁移到子模块

### 阶段结果判断

到阶段 6 为止，大型测试入口已经从“单文件承载多类场景”收敛为“薄入口 + 主题测试模块”结构：

- `grpc_proxy.rs` 只保留共享 fixture、helper 和模块声明
- `admin.rs`、`reload.rs`、`downstream_mtls.rs` 都按场景主题拆成子模块
- `proxy/tests/mod.rs` 只保留共享 fixture、helper 和模块声明，header/grpc/client/peer 行为已经分离

下一步适合进入阶段 7，处理工具脚本和长文档的拆分。

## 阶段 7：工具与文档拆分

### 目标

把工具脚本和长文档从“一次性产物”改成可持续维护结构。

### 目标文件

- [scripts/nginx_compare.py](../scripts/nginx_compare.py)
- [docs/nginx-comparison.md](./nginx-comparison.md)

### 建议拆法

`nginx_compare.py`：

- `nginx_compare/checkout.py`
- `nginx_compare/build.py`
- `nginx_compare/launch.py`
- `nginx_compare/scenarios.py`
- `nginx_compare/render.py`
- `nginx_compare/main.py`

文档：

- 主文档只保留结论、方法、边界
- 一次性 benchmark 样本移到带日期的快照文档

### 完成标准

- 工具链变更不再集中在单个长脚本
- 长期文档不再绑定一次运行结果

### 当前执行结果

已完成。

本阶段已做：

- 将 [scripts/nginx_compare.py](../scripts/nginx_compare.py) 收敛为兼容入口脚本
- 将对比工具目录化为：
  - `scripts/nginx_compare/common.py`
  - `scripts/nginx_compare/checkout.py`
  - `scripts/nginx_compare/build.py`
  - `scripts/nginx_compare/configs.py`
  - `scripts/nginx_compare/launch.py`
  - `scripts/nginx_compare/render.py`
  - `scripts/nginx_compare/scenarios.py`
  - `scripts/nginx_compare/main.py`
- 保持现有 CLI 入口不变：
  - `scripts/nginx_compare.py`
  - `scripts/run-nginx-compare-docker.sh`
- 将 [docs/nginx-comparison.md](./nginx-comparison.md) 收敛为长期结论、方法和边界文档
- 将一次性 smoke 样本迁移到：
  - [docs/nginx-comparison-snapshots/2026-04-10-trixie-smoke.md](./nginx-comparison-snapshots/2026-04-10-trixie-smoke.md)

### 阶段结果判断

到阶段 7 为止，工具脚本和长文档已经从“一次性长文件”收敛为“薄入口 + 子模块 / 主文档 + 快照文档”结构：

- `scripts/nginx_compare.py` 不再承载 checkout/build/launch/render/main 的全部实现
- benchmark 样本不再直接绑定在长期对比文档里
- 后续如果要继续迭代对比 harness 或增加新一轮性能快照，都可以局部修改，不需要回到单个超长文件

## 每阶段门禁

### 通用门禁

- `cargo fmt --all`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

### 按阶段追加门禁

阶段 1~3：

- `./scripts/test-fast.sh`

阶段 4~6：

- `./scripts/test-fast.sh`
- 受影响模块的定向集成测试

阶段 7：

- `python3 -m py_compile scripts/nginx_compare.py`
- 文档内链检查

## 风险点

### 1. 拆分时顺手改语义

这是最常见的风险。

应对：

- 每个 PR 只允许“结构整理 + 最少必要修复”

### 2. 入口文件 re-export 变化导致外部调用断裂

应对：

- 拆分阶段尽量保持公开函数路径不变

### 3. 测试 helper 先拆导致生产代码边界继续恶化

应对：

- 测试拆分放在生产边界整理之后

### 4. 热路径拆分导致性能或行为回归

应对：

- `proxy` / `handler` / `runtime bootstrap` 放到后期
- 每一步都保留定向回归

## 建议执行顺序

推荐实际执行顺序：

1. 阶段 1：`compile.rs`、`state.rs`、`tls.rs`
2. 阶段 2：`admin_cli.rs`、`migrate_nginx.rs`
3. 阶段 3：`tls_runtime.rs`
4. 阶段 4：`bootstrap.rs`、`server.rs`、`timeout.rs`
5. 阶段 5：`handler` / `proxy`
6. 阶段 6：大型测试文件
7. 阶段 7：工具与文档

## 当前建议

如果现在开始动手，最合理的起点是：

- 先执行“阶段 1”

原因：

- 风险最低
- 收益最高
- 对后续阶段有放大作用
- 不会立刻碰最复杂的协议热路径
