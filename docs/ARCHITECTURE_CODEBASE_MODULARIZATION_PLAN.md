# rginx Codebase Modularization Plan

本文档定义仓库级“大文件拆分 / 单文件单职责”重构计划。

目标不是做功能升级，而是在不改变外部行为的前提下，把当前已经开始累积维护压力的大文件拆成边界清晰、职责单一、可独立演进的模块。

## 目标

- 每个 `.rs` 文件只承载一个明确模块职责。
- `lib.rs`、`main.rs`、目录门面文件只做 `mod` 声明、`pub use` 和极薄分发。
- 生产代码文件不再内嵌 `mod tests`。
- 高风险协议逻辑拆成目录模块，避免继续向单文件堆叠。
- 拆分过程默认保持零行为变化。

## 非目标

- 不在本计划内引入新功能。
- 不主动调整现有协议语义、配置语义或管理面输出格式。
- 不以“简化目录数”为目标；优先追求边界清晰。
- 不在早期阶段顺手重写高风险逻辑。

## 当前现状

基于当前仓库静态分析：

- 共有 6 个 crate。
- 生产代码中有 22 个文件超过 500 行。
- 测试代码中有 27 个文件超过 300 行。
- 维护压力主要集中在 `rginx-http`、`rginx-app`、`rginx-config`。

当前最需要拆分的生产文件包括：

- `crates/rginx-http/src/tls/ocsp/mod.rs`
- `crates/rginx-http/src/proxy/health/registry.rs`
- `crates/rginx-http/src/server/http3.rs`
- `crates/rginx-app/src/main.rs`
- `crates/rginx-http/src/proxy/clients/http3.rs`
- `crates/rginx-app/src/admin_cli/status.rs`
- `crates/rginx-runtime/src/bootstrap/listeners.rs`
- `crates/rginx-config/src/compile/server.rs`
- `crates/rginx-config/src/validate/server.rs`
- `crates/rginx-http/src/compression.rs`
- `crates/rginx-http/src/proxy/request_body.rs`
- `crates/rginx-http/src/state/counters.rs`
- `crates/rginx-http/src/state/snapshots.rs`
- `crates/rginx-http/src/proxy/forward/mod.rs`
- `crates/rginx-http/src/handler/dispatch.rs`

## 重构规则

### 文件与模块规则

- 一个文件只承载一个模块职责。
- 一个目录可以包含一个模块及其子模块。
- `mod.rs` 可以保留，但只允许做门面，不承载重逻辑。
- 新增目录时，优先使用“门面文件 + 子目录”的形式。

### 测试规则

- 生产文件中的 `mod tests` 全部迁出。
- 单元测试保留在同级 `tests/` 目录或拆分后的测试文件中。
- 大型集成测试文件按场景拆目录，不再继续向单文件追加 case。

### 风险控制规则

- 拆分顺序遵循“低风险先行，高风险后拆”。
- 每个阶段优先做结构调整，不主动修改业务语义。
- 每次拆分只跨一个 crate 或一个高内聚模块群，避免大爆炸提交。

## 分阶段计划

### 阶段 0：建立规则与门禁

目标：

- 定义仓库内的模块拆分约束。
- 给后续重构建立静态检查和 review 基线。

产出：

- 明确文件大小软上限和硬上限。
- 明确 `main.rs`、`lib.rs`、`mod.rs` 的职责限制。
- 明确禁止在生产文件中继续新增 `mod tests`。
- 为后续拆分建立约定文档和 CI 检查入口。
- 阶段 0 的落地文件：
  - `docs/ARCHITECTURE_CODEBASE_MODULARIZATION_POLICY.md`
  - `scripts/modularization_baseline.json`
  - `scripts/run-modularization-gate.py`

完成标准：

- 新提交不再继续放大现有大文件。
- 团队对“单文件单职责”的定义达成一致。

### 阶段 1：机械化拆分测试与内嵌模块

目标：

- 先清理最容易机械化迁移的结构问题。

范围：

- 迁出所有生产文件内嵌测试。
- 把大型测试文件拆成场景目录和辅助文件。
- 清理目录中的“门面 + 实现 + 测试”混装问题。

优先对象：

- `crates/rginx-app/tests/http3.rs`
- `crates/rginx-app/tests/grpc_proxy.rs`
- `crates/rginx-app/tests/ocsp.rs`
- `crates/rginx-app/tests/check.rs`
- `crates/rginx-http/src/*/tests.rs`

完成标准：

- 生产文件不再包含 `mod tests`。
- 集成测试场景按功能拆分后仍通过现有测试矩阵。

### 阶段 2：拆分 rginx-app

目标：

- 把入口层、检查输出层、admin CLI 输出层分离。

重点文件：

- `crates/rginx-app/src/main.rs`
- `crates/rginx-app/src/admin_cli/status.rs`
- `crates/rginx-app/src/admin_cli/traffic.rs`

建议拆分：

- `main.rs` 只保留真正入口和命令分发。
- 新增 `check/summary.rs`、`check/render.rs`、`check/tls.rs`、`check/routes.rs`、`signal.rs`、`pid_file.rs`、`runtime.rs`。
- `admin_cli/status.rs` 拆成 `status/runtime.rs`、`status/listeners.rs`、`status/tls.rs`、`status/upstream_tls.rs`。

完成标准：

- `main.rs` 变成薄入口文件。
- `admin_cli` 每个输出域有独立模块文件。

### 阶段 3：拆分 rginx-config

状态：

- 已完成（2026-04-26）

目标：

- 把配置模型、预处理、校验、编译按领域拆开。

重点文件：

- `crates/rginx-config/src/model.rs`
- `crates/rginx-config/src/load.rs`
- `crates/rginx-config/src/validate/server.rs`
- `crates/rginx-config/src/compile/server.rs`
- `crates/rginx-config/src/compile/upstream.rs`

建议拆分：

- `model.rs` 拆为 `model/runtime.rs`、`model/server.rs`、`model/listener.rs`、`model/route.rs`、`model/upstream.rs`、`model/tls.rs`、`model/vhost.rs`。
- `load.rs` 拆为 `load/include.rs`、`load/preprocess.rs`、`load/env_expand.rs`、`load/parse.rs`。
- `validate/server.rs` 拆为 listener 规则、server_name 规则、TLS/HTTP3 规则。
- `compile/server.rs` 拆为 listener 编译、TLS 编译、HTTP3 编译、access_log 编译。

完成标准：

- `load/validate/compile` 三层都不再依赖单一大文件承载主逻辑。
- 新配置项能在清晰分层下落位。

实际落地：

- `model.rs` 已拆为 `model/` 下按领域组织的门面 + 子模块。
- `load.rs` 已拆为 `load/include.rs`、`load/preprocess.rs`、`load/env_expand.rs`、`load/parse.rs`。
- `validate/server.rs` 已拆为 listener、TLS、HTTP/3、server_name、trusted_proxies 等独立子模块。
- `compile/server.rs` 已拆为 listener、fields、http3、tls 及其子模块。
- `compile/upstream.rs` 已拆为 dns、peer、settings、tls 子模块。

### 阶段 4：拆分 rginx-runtime

状态：

- 已完成（2026-04-26）

目标：

- 把 listener 生命周期、OCSP 后台任务、管理面运行时职责分开。

重点文件：

- `crates/rginx-runtime/src/bootstrap/listeners.rs`
- `crates/rginx-runtime/src/ocsp.rs`
- `crates/rginx-runtime/src/admin.rs`

建议拆分：

- `bootstrap/listeners.rs` 拆为 `group.rs`、`bind_tcp.rs`、`bind_udp.rs`、`prepare.rs`、`activate.rs`、`reconcile.rs`、`drain.rs`、`join.rs`。
- `ocsp.rs` 拆为 `spec.rs`、`scheduler.rs`、`refresh.rs`、`persist.rs`、`state.rs`。

完成标准：

- listener reload/restart/drain 路径可按文件级职责阅读。
- runtime 编排逻辑不再集中在单个“总装文件”。

实际落地：

- `bootstrap/listeners.rs` 已拆为 `listeners/` 目录下的 `group`、`bind_tcp`、`bind_udp`、`prepare`、`activate`、`reconcile`、`drain`、`join` 子模块。
- `ocsp.rs` 已拆为 `ocsp/` 目录下的 `client`、`spec`、`state`、`refresh`、`persist`、`scheduler` 子模块。
- `admin.rs` 已拆为 `admin/` 目录下的 `model`、`socket`、`service` 子模块。

### 阶段 5：拆分 rginx-http 的通用路径

状态：

- 已完成（2026-04-26）

目标：

- 先处理 HTTP 数据面里风险较低但体量已重的通用模块。

重点文件：

- `crates/rginx-http/src/handler/dispatch.rs`
- `crates/rginx-http/src/handler/grpc.rs`
- `crates/rginx-http/src/compression.rs`
- `crates/rginx-http/src/proxy/forward/mod.rs`
- `crates/rginx-http/src/proxy/request_body.rs`
- `crates/rginx-http/src/state/counters.rs`
- `crates/rginx-http/src/state/snapshots.rs`

建议拆分：

- `dispatch.rs` 拆为 request metadata、route selection、authorization、response finalization。
- `compression.rs` 拆为 policy、accept_encoding、content_type、encoder。
- `forward/mod.rs` 拆为 orchestration、retry、timeouts、upgrade、error mapping。
- `request_body.rs` 拆为 buffering、streaming、replayability、limits。
- `counters.rs` 与 `snapshots.rs` 拆为 listener/vhost/route/upstream/tls/http3 等独立结构文件。

完成标准：

- 通用数据面路径不再依赖“大总调度文件”。
- 观测结构和行为逻辑从文件层面解耦。

实际落地：

- `compression.rs` 已拆为 `compression/` 目录下的 `options`、`accept_encoding`、`content_type`、`encode` 子模块，`mod.rs` 只保留压缩编排入口。
- `handler/dispatch.rs` 已拆为 `dispatch/` 目录下的 `authorize`、`select`、`response`、`date`、`route` 子模块，主文件只保留请求主流程。
- `handler/grpc.rs` 已拆为 `grpc/` 目录下的 `metadata`、`error`、`observability`、`grpc_web` 子模块。
- `proxy/forward/mod.rs` 已拆为 `forward/` 目录下的 `types`、`setup`、`attempt`、`streaming` 子模块，建立显式的下游准备和上游尝试边界。
- `proxy/request_body.rs` 已拆为 `request_body/` 目录下的 `model`、`prepare`、`replay`、`streaming`、`limits` 子模块。
- `state/snapshots.rs` 已拆为 `snapshots/` 目录下的 `active`、`http`、`tls`、`reload`、`delta`、`runtime`、`upstreams`、`traffic` 子模块。
- `state/counters.rs` 已拆为 `counters/` 目录下的 `http`、`rolling`、`grpc`、`traffic`、`upstreams`、`versions` 分段文件，并从 `state/mod.rs` 显式聚合。

### 阶段 6：拆分高风险协议模块

目标：

- 最后处理 QUIC/HTTP3、健康状态机、OCSP 这类协议密集区。

重点文件：

- `crates/rginx-http/src/server/http3.rs`
- `crates/rginx-http/src/proxy/clients/http3.rs`
- `crates/rginx-http/src/proxy/health/registry.rs`
- `crates/rginx-http/src/tls/ocsp/mod.rs`

建议拆分：

- `server/http3.rs` 拆为 `body.rs`、`endpoint.rs`、`host_key.rs`、`accept_loop.rs`、`connection.rs`、`request.rs`、`response.rs`、`close_reason.rs`。
- `proxy/clients/http3.rs` 拆为 `endpoint_cache.rs`、`session.rs`、`connect.rs`、`request.rs`、`response_body.rs`。
- `proxy/health/registry.rs` 拆为 `policy.rs`、`state.rs`、`selection.rs`、`endpoint_map.rs`、`snapshot.rs`、`guards.rs`。
- `tls/ocsp/mod.rs` 拆为 `discover.rs`、`request.rs`、`nonce.rs`、`validate.rs`、`signer.rs`、`time.rs`、`der_helpers.rs`。

完成标准：

- 协议处理、状态机、序列化辅助、I/O 桥接不再混装在单一文件。
- 高风险模块能按子域独立 review 和回归。

### 阶段 7：收口与长期治理

目标：

- 防止拆完后重新长回去。

产出：

- 统一命名和目录规范。
- 为超大文件回潮增加 CI 检查。
- 补齐模块级文档。
- 清理历史遗留的重逻辑门面文件。

完成标准：

- 后续新增功能默认按模块目录落位。
- 仓库不再依赖几个“核心超大文件”承载持续演化。

## 建议执行顺序

建议按以下顺序推进，而不是按行数从大到小硬拆：

1. 阶段 0
2. 阶段 1
3. 阶段 2
4. 阶段 3
5. 阶段 4
6. 阶段 5
7. 阶段 6
8. 阶段 7

原因：

- 先做低风险结构清理，减少后续协议重构噪音。
- 先把入口层、配置层、运行时层切薄，再处理 HTTP/3 / OCSP 这种高风险协议代码。
- 先建立测试外置和门面薄化规则，再拆最难的核心模块。

## 每阶段验收建议

最低回归要求：

- `./scripts/test-fast.sh`

涉及集成行为时：

- `./scripts/test-slow.sh`

涉及 TLS / HTTP3 / OCSP 时：

- `./scripts/run-tls-gate.sh`
- `./scripts/run-http3-gate.sh`

涉及发布收口变更时：

- `./scripts/run-http3-release-gate.sh --soak-iterations 1`

## 交付建议

- 不做单个超大 PR。
- 每个阶段拆成多个小 PR，按 crate 或模块群推进。
- 默认每个 PR 只做结构重组和必要的 `use`/路径调整。
- 协议高风险阶段优先保证目录边界清晰，再决定是否继续细分实现。

## 计划结论

这项工作适合按“仓库治理”项目推进，而不是按“单文件修修补补”推进。

真正的目标不是单次把文件变短，而是让未来新增代码没有理由再回到“一个文件承载多个模块职责”的状态。
