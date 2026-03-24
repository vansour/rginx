# Refactor Plan

本页记录 `rginx` 当前模块化重构的阶段基线。

阶段 0 的目标不是改行为，而是先把“哪些东西不能动”“后续应该怎么拆”写清楚，避免后续拆文件时一边改结构一边改语义。

## 阶段 0 输出

阶段 0 完成后，仓库内应明确以下三件事：

1. 对外边界保持不变。
2. `rginx-http` 内部的自然拆分方向已经固定。
3. 后续阶段按文件迁移时，有明确的函数和类型落点清单。

## 不变约束

后续阶段 1 / 2 / 3 都应遵守下面这些约束：

- 不改 crate 边界：
  - `rginx-app`
  - `rginx-config`
  - `rginx-core`
  - `rginx-http`
  - `rginx-runtime`
  - `rginx-observability`
- 不改二进制入口调用链：
  - `rginx-app -> rginx-config -> rginx-runtime -> rginx-http`
- 不改 `rginx-http` 在 [crates/rginx-http/src/lib.rs](../crates/rginx-http/src/lib.rs) 的对外暴露形状：
  - `pub mod handler`
  - `pub mod metrics`
  - `pub mod proxy`
  - `pub mod rate_limit`
  - `pub mod router`
  - `pub mod server`
  - `pub mod state`
  - `pub use server::serve`
  - `pub use state::SharedState`
- 不改核心公开入口名字和职责：
  - `handler::handle`
  - `proxy::forward_request`
  - `proxy::probe_upstream_peer`
  - `server::serve`
  - `state::SharedState`
- 不改配置语义、热重载语义、日志语义、指标语义。
- 不在“拆文件”阶段顺手加入新功能。
- 不修改现有测试用例结构与用例名；每一阶段都要求 `cargo test --workspace` 通过。

## 导出策略

`rginx-http` 后续的拆分方式，不是把大文件直接对外拆成更多顶层模块，而是在原有模块名下改成目录模块。

也就是说：

- 现在的 `crates/rginx-http/src/proxy.rs`
  - 后续变成 `crates/rginx-http/src/proxy/mod.rs`
- 现在的 `crates/rginx-http/src/handler.rs`
  - 后续变成 `crates/rginx-http/src/handler/mod.rs`

这样做的目的：

- 外部 `use rginx_http::proxy::...` 路径不变
- 外部 `use rginx_http::handler::...` 路径不变
- 只把模块内部实现拆散，不把调用方一起拖进重构

内部可见性约束：

- 新拆出的子模块默认使用 `pub(crate)` 或私有项
- 只有 `proxy/mod.rs` 和 `handler/mod.rs` 负责对外 re-export
- 不新增新的顶层 `pub mod ...`，除非后续有明确 API 需要

## 目标目录

阶段 1 和阶段 2 完成后的自然目录如下：

```text
crates/rginx-http/src/
  lib.rs
  server.rs
  router.rs
  client_ip.rs
  rate_limit.rs
  metrics.rs
  state.rs
  compression.rs
  file.rs
  timeout.rs
  tls.rs
  proxy/
    mod.rs
    clients.rs
    health.rs
    grpc_web.rs
    request_body.rs
    forward.rs
    upgrade.rs
  handler/
    mod.rs
    dispatch.rs
    access_log.rs
    grpc.rs
    admin.rs
```

## 阶段 1：`proxy.rs` 迁移清单

### `proxy/mod.rs`

保留或 re-export：

- `ProxyClient`
- `DownstreamRequestOptions`
- `ProxyClients`
- `PeerStatusSnapshot`
- `probe_upstream_peer`
- `forward_request`

职责：

- 聚合子模块
- 对外稳定导出

### `proxy/clients.rs`

迁移类型：

- `ProxyClient`
- `UpstreamClientProfile`
- `ProxyClients`
- `InsecureServerCertVerifier`

迁移函数：

- `build_client_for_profile`
- `build_tls_config`
- `load_custom_ca_store`

### `proxy/health.rs`

迁移类型：

- `PeerHealthPolicy`
- `PeerHealthKey`
- `PeerFailureStatus`
- `PassiveHealthState`
- `ActiveHealthState`
- `PeerHealthState`
- `ActiveProbeStatus`
- `PeerHealth`
- `PeerHealthRegistry`
- `SelectedPeers`
- `PeerStatusSnapshot`
- `ActivePeerGuard`
- `ActivePeerBody`

迁移函数：

- `lock_peer_health`
- `merge_selected_peers`
- `projected_least_conn_load`
- `probe_upstream_peer`
- `build_active_health_request`
- `evaluate_grpc_health_probe_response`
- `collect_response_body_and_trailers`
- `grpc_trailer_value`
- `encode_grpc_health_check_request`
- `decode_grpc_health_check_response`
- `decode_grpc_frame_payload`
- `decode_grpc_health_check_response_payload`
- `append_protobuf_varint`
- `decode_protobuf_varint`
- `skip_protobuf_field`
- `invalid_grpc_health_probe`
- `grpc_health_serving_status_label`

### `proxy/grpc_web.rs`

迁移类型：

- `GrpcWebMode`
- `GrpcWebEncoding`
- `GrpcWebResponseBody`
- `GrpcWebTextDecodeBody`
- `GrpcWebTextEncodeBody`
- `GrpcWebRequestBody`
- `ParsedGrpcWebRequestFrame`

迁移函数：

- `detect_grpc_web_mode`
- `split_content_type`
- `decode_grpc_web_request_frame`
- `decode_grpc_web_trailer_block`
- `decode_grpc_web_text_chunk`
- `decode_grpc_web_text_final`
- `encode_grpc_web_text_chunk`
- `flush_grpc_web_text_chunk`
- `extract_grpc_initial_trailers`
- `encode_grpc_web_trailers`
- `invalid_grpc_web_body`
- `append_header_map`

### `proxy/request_body.rs`

迁移类型：

- `PreparedProxyRequest`
- `PreparedRequestBody`
- `CollectedRequestBody`
- `PrepareRequestError`
- `ReplayableRequestBody`

迁移函数：

- `prepare_request_body`
- `collect_request_body`
- `downstream_request_body`
- `is_idempotent_method`
- `can_retry_peer_request`
- `upstream_request_version`

### `proxy/forward.rs`

迁移类型：

- `GrpcResponseDeadline`

迁移函数：

- `forward_request`
- `wait_for_upstream_stage`
- `gateway_timeout`
- `grpc_timeout_message`
- `bad_gateway`
- `payload_too_large`
- `bad_request`
- `unsupported_media_type`
- `grpc_protocol_request`
- `effective_upstream_request_timeout`
- `parse_grpc_timeout`
- `grpc_timeout_duration`
- `build_downstream_response`
- `build_proxy_uri`
- `sanitize_request_headers`
- `preserved_te_trailers_value`
- `sanitize_response_headers`
- `remove_hop_by_hop_headers`
- `is_upgrade_request`
- `is_upgrade_response`
- `extract_upgrade_protocol`
- `connection_header_contains_token`

### `proxy/upgrade.rs`

迁移类型：

- `ActiveConnectionGuard`

迁移函数：

- `proxy_upgraded_connection`

## 阶段 2：`handler.rs` 迁移清单

### `handler/mod.rs`

保留或 re-export：

- `handle`
- `grpc_error_response`
- `text_response`
- `full_body`

### `handler/dispatch.rs`

迁移函数：

- `handle`
- `select_vhost_for_request`
- `route_match_context`
- `select_route_for_request`
- `authorize_route`
- `enforce_rate_limit`
- `build_route_response`
- `response_body_bytes_sent`
- `request_host`
- `header_value`
- `http_version_label`
- `strip_response_body`

### `handler/access_log.rs`

迁移类型：

- `AccessLogContext`
- `OwnedAccessLogContext`

迁移函数：

- `log_access_event`
- `render_access_log_line`

### `handler/grpc.rs`

迁移类型：

- `GrpcObservability`
- `GrpcRequestMetadata`
- `GrpcStatusCode`
- `GrpcResponseFormat`
- `GrpcResponseFinalizer`
- `GrpcAccessLogBody`
- `GrpcWebObservabilityParser`
- `ParsedGrpcWebObservabilityFrame`

迁移函数：

- `grpc_observability`
- `grpc_request_metadata`
- `grpc_protocol`
- `grpc_response_format`
- `grpc_error_response`
- `build_grpc_error_response`
- `sanitize_grpc_message`
- `encode_grpc_web_error_body`
- `wrap_grpc_observability_response`
- `decode_grpc_web_observability_frame`
- `decode_grpc_web_trailer_block_for_observability`
- `decode_grpc_web_text_observability_chunk`
- `decode_grpc_web_text_observability_final`
- `invalid_grpc_observability`
- `grpc_service_method`
- `split_header_content_type`

### `handler/admin.rs`

迁移类型：

- `StatusPayload`
- `ConfigPayload`
- `UpstreamStatusPayload`
- `ActiveHealthCheckPayload`
- `ErrorPayload`

迁移函数：

- `config_response`
- `config_state_response`
- `config_update_response`
- `status_response`
- `metrics_response`
- `forbidden_response`
- `too_many_requests_response`
- `text_response`
- `json_response`
- `json_error_response`
- `method_not_allowed_response`
- `full_body`
- `health_check_payload`

## 阶段完成标准

阶段 0 只要求形成基线，不要求代码结构已经拆完。

以下条件满足时，可认为阶段 0 完成：

- 重构约束已经在仓库内落成文档
- `proxy.rs` 和 `handler.rs` 的子模块目标已经固定
- 阶段 1 / 2 的迁移清单已经写明
- 当前仓库在未重构行为的前提下仍保持测试通过

## 执行建议

建议实际执行顺序：

1. 先做阶段 1，拆 `proxy`
2. 再做阶段 2，拆 `handler`
3. 完成后再评估是否继续拆 `rginx-config/src/compile.rs` 和 `validate.rs`
