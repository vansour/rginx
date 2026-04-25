# HTTP/3 NGINX Alignment Plan

状态：已完成，当前收口到 `v0.1.3-rc.13`。

这个文档保留 HTTP/3 主线的阶段化设计口径。它现在是归档性的计划文档，用来说明这条主线当时要交付什么、哪些能力被视为完成、以及今天发布门禁依赖哪些资产。

## 目标

- 让 `rginx` 在入口反代场景下具备可发布的下游 HTTP/3 能力。
- 保持现有配置模型、TLS 终止、管理面和 reload/restart 语义的一致性。
- 把 HTTP/3 从“功能存在”推进到“有专门回归门禁、可被运维观察、可纳入发布流程”。

## 非目标

- 不追求 nginx 全量 DSL 兼容。
- 不做通用 QUIC 平台或独立传输层框架。
- 不把 HTTP/3 作为隐式自动升级路径；配置仍然坚持显式启用。

## 阶段划分

### Phase 0

- 确认基线：
  - 既有 HTTP/1.1 / HTTP/2 入口路径可用
  - TLS、admin、reload/restart、集成测试基建已存在
  - HTTP/3 仍未进入常规测试和发布门禁
- 详细见 `HTTP3_PHASE0_BASELINE.md`

### Phase 1

- 下游 HTTP/3 ingress 建立
- `Return` 与基础 `Proxy` 在 HTTP/3 下可用
- `Alt-Svc` 广播打通

### Phase 2

- 补齐 HTTP/3 下的中间件与入口语义：
  - ACL
  - 限流
  - 压缩
  - request-id
  - access log
  - traffic 统计

### Phase 3

- 建立上游 HTTP/3 基础代理路径
- 支持显式 `protocol: Http3`
- 补齐 `server_name_override` 和 client identity 等 TLS 侧能力

### Phase 4

- 打通 gRPC over HTTP/3
- 打通 grpc-web over HTTP/3
- 保持 deadline、trailers 和主动健康检查语义

### Phase 5

- 把 TLS / SNI / OCSP 诊断扩到 HTTP/3 listener
- 让管理面、`check`、`status`、`snapshot` 能反映 HTTP/3 绑定和运行态

### Phase 6

- 明确 reload / restart / drain 语义
- 加入 0-RTT 与 replay-safe route 策略
- 补 QUIC runtime telemetry

### Phase 7

- 把 HTTP/3 纳入独立 regression gate
- 引入 focused soak
- 把 release-prep 与 GitHub release workflow 接到 HTTP/3 release gate
- 详细见 `HTTP3_PHASE7_RELEASE.md`

## 当前完成口径

当前仓库把下列内容视为 HTTP/3 主线的完成条件：

- 下游 HTTP/3 ingress、TLS / QUIC / Alt-Svc
- HTTP/3 下的 `Return` / `Proxy`
- ACL / 限流 / 压缩 / request-id / access log / traffic 统计
- 上游 HTTP/3 代理
- gRPC / grpc-web over HTTP/3
- 0-RTT 与 replay-safe route 策略
- TLS / SNI / OCSP 诊断
- reload / restart / drain
- QUIC runtime telemetry
- dedicated gate、focused soak、release gate

## 发布验证资产

HTTP/3 主线当前依赖的主要验证入口：

- `scripts/run-http3-gate.sh`
- `scripts/run-http3-soak.sh`
- `scripts/run-http3-release-gate.sh`
- `crates/rginx-app/tests/http3.rs`
- `crates/rginx-app/tests/upstream_http3.rs`
- `crates/rginx-app/tests/grpc_http3.rs`
- `crates/rginx-app/tests/reload.rs`
- `crates/rginx-app/tests/admin.rs`
- `crates/rginx-app/tests/check.rs`
- `crates/rginx-app/tests/ocsp.rs`

## 维护要求

- 如果新增 HTTP/3 控制面字段，必须同步更新 `admin` / `check` 相关测试和发布说明。
- 如果变更 HTTP/3 发布门禁组成，必须同步更新 `HTTP3_PHASE7_RELEASE.md` 与 README。
