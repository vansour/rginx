# Upstream HTTP/3 Production Plan

状态：已完成，能力已并入主线 HTTP/3 发布门禁。

这个文档保留“上游 HTTP/3 生产级传输”专项的设计口径。主线 HTTP/3 更关注入口能力；这个专项当时关注的是把上游 HTTP/3 从“能连通”推进到“可在真实代理链路里稳定使用”。

## 目标

- 为 upstream 提供显式 `protocol: Http3`
- 保持 TLS1.3、SNI、证书校验和 client identity 行为可控
- 支持普通 HTTP、gRPC、grpc-web 和主动健康检查
- 让上游 HTTP/3 的状态能被 control plane 和发布门禁覆盖

## 非目标

- 不做自动协议探测或自动升级
- 不对单个 upstream 混用多种传输语义做复杂协商
- 不追求浏览器式连接迁移或通用 QUIC 终端能力

## 阶段划分

### Phase 0

- 记录基线和约束
- 详细见 `ARCHITECTURE_UPSTREAM_HTTP3_PHASE0_BASELINE.md`

### Phase 1

- 建立基础 HTTP/3 upstream client 路径
- 支持普通 HTTP 代理请求转发

### Phase 2

- 补齐 TLS 名称与身份策略：
  - `server_name`
  - `server_name_override`
  - client certificate / key
  - trust anchor / verify depth

### Phase 3

- 补齐 gRPC over HTTP/3 upstream
- 补齐 grpc-web over HTTP/3 upstream
- 保持 deadline、trailers 和错误映射语义

### Phase 4

- 把主动 gRPC health check 扩到 HTTP/3 upstream
- 让 peer health 与请求选择逻辑感知 HTTP/3 upstream 状态

### Phase 5

- 收口连接复用、reload 配合、控制面可见性和发布门禁
- 把上游 HTTP/3 纳入 `run-http3-gate.sh`、soak 和 release gate

## 当前完成口径

当前仓库中，上游 HTTP/3 已具备：

- 显式 `protocol: Http3`
- 普通 HTTP 请求代理
- `server_name_override`
- client identity
- gRPC / grpc-web over HTTP/3
- 主动 gRPC health check
- 进入 dedicated gate、focused soak 与 release gate

## 主要验证入口

- `crates/rginx-app/tests/upstream_http3.rs`
- `crates/rginx-app/tests/grpc_http3.rs`
- `crates/rginx-app/tests/admin.rs`
- `crates/rginx-app/tests/check.rs`
- `scripts/run-http3-gate.sh`
- `scripts/run-http3-soak.sh`

## 维护要求

- 变更 upstream HTTP/3 的证书校验、SNI 或 client identity 逻辑时，必须同步看 `upstream_http3.rs`。
- 变更 upstream HTTP/3 的 gRPC 或 health 行为时，必须同步看 `grpc_http3.rs`。
