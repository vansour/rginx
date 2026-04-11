# x509-parser Stage 0 Baseline

更新时间：`2026-04-11`

## 目标

Stage 0 的目标不是改实现，而是冻结当前 `x509-parser` 相关生产行为，确保后续
`x509-parser -> rasn` 的迁移有明确回归标准。

这份文档回答三个问题：

1. 当前 `x509-parser` 相关逻辑对外承诺了什么行为
2. 这些行为现在由哪些测试覆盖
3. 后续阶段至少要跑哪些门禁，才能认为迁移没有回退

## 行为基线

当前最小行为承诺如下。

### 证书巡检与运行态展示

来自 [certificates.rs](../crates/rginx-http/src/state/tls_runtime/certificates.rs)：

- 能读取 leaf 证书的 `subject / issuer / serial`
- 能读取 SAN DNS names
- 能输出 SHA-256 fingerprint
- 能读取并展示：
  - SKI
  - AKI
  - BasicConstraints
  - KeyUsage
  - ExtendedKeyUsage
  - validity 时间
- 能生成链路诊断：
  - 过期 / 即将过期
  - 链不完整
  - issuer/subject link mismatch
  - AKI/SKI mismatch
  - leaf 缺少 `server_auth` EKU

### 下游 mTLS 客户端身份提取

来自 [connection.rs](../crates/rginx-http/src/server/connection.rs)：

- 能从下游客户端证书链提取：
  - leaf `subject`
  - leaf `issuer`
  - leaf `serial`
  - leaf SAN DNS names
  - chain length
  - chain subjects，且顺序保持为 leaf -> issuer -> 更高层

### CRL DER 校验

来自 [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)：

- PEM CRL 可以被正常加载
- DER CRL 可以被正常加载
- DER CRL 若包含 trailing data，会被拒绝

### OCSP 行为

来自 [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 和
[ocsp.rs](../crates/rginx-app/tests/ocsp.rs)：

- 能从证书链构造 OCSP request
- 能从证书 AIA 中提取 responder URL
- 只接受 `Successful` 的 OCSP response status
- 只接受匹配当前证书 `CertId` 的 `SingleResponse`
- 保持当前 responder / nonce / signature / validity 校验语义
- 无效或过期的 cache 文件不会继续被 stapling
- invalid refresh response 不会被写入 cache
- expired cache 在 refresh 失败后会被清空

### 外部输出

当前这些外部输出已被视为基线：

- `rginx check`
  - TLS 证书摘要
  - OCSP 摘要
- `rginx status`
  - TLS certificate records
  - TLS vhost/SNI binding records
  - upstream TLS records
- mTLS access log 变量：
  - `$tls_client_subject`
  - `$tls_client_issuer`
  - `$tls_client_serial`
  - `$tls_client_san_dns_names`
  - `$tls_client_chain_length`
  - `$tls_client_chain_subjects`

## 当前自动化覆盖

### 证书巡检

来自 [state/tests.rs](../crates/rginx-http/src/state/tests.rs)：

- `inspect_certificate_reports_fingerprint_and_incomplete_chain_diagnostics`
  - 冻结 fingerprint、subject/issuer、SAN、单证书链不完整诊断
- `inspect_certificate_reports_aki_ski_and_server_auth_eku_diagnostics`
  - 冻结 AKI/SKI、EKU 读取与 `leaf_missing_server_auth_eku`

来自 [admin/snapshot.rs](../crates/rginx-app/tests/admin/snapshot.rs)：

- `status_and_snapshot_commands_report_tls_certificate_and_binding_diagnostics`
  - 冻结 admin snapshot 与 `rginx status` 中 TLS certificate / vhost binding / SNI binding 输出

### mTLS 身份提取与外部可见性

来自 [server/tests.rs](../crates/rginx-http/src/server/tests.rs)：

- `parse_tls_client_identity_extracts_subject_and_dns_san`
  - 冻结单证书身份提取
- `parse_tls_client_identity_preserves_leaf_fields_and_chain_order`
  - 冻结多证书链的 leaf 字段与 chain subject 顺序

来自 [downstream_mtls/observability.rs](../crates/rginx-app/tests/downstream_mtls/observability.rs)：

- `mtls_access_log_variables_render_client_identity`
  - 冻结 mTLS access log 变量的渲染内容

### CRL

来自 [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)：

- `load_certificate_revocation_lists_accepts_der_without_trailing_data`
  - 冻结 DER CRL 正常加载语义
- `load_certificate_revocation_lists_rejects_der_with_trailing_data`
  - 冻结 DER trailing data 拒绝语义

来自 [downstream_mtls/validation.rs](../crates/rginx-app/tests/downstream_mtls/validation.rs)：

- `required_mtls_rejects_revoked_client_certificates_via_crl`
  - 冻结运行时 CRL 实际生效语义

### OCSP

来自 [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)：

- `validate_ocsp_response_matches_current_certificate`
- `validate_ocsp_response_rejects_expired_response`
- `load_certified_key_bundle_ignores_stale_ocsp_cache`
- `validate_ocsp_response_rejects_future_produced_at`
- `validate_ocsp_response_rejects_unknown_certificate_status`
- `validate_ocsp_response_rejects_invalid_signature`
- `validate_ocsp_response_accepts_authorized_delegated_signer`
- `validate_ocsp_response_rejects_delegated_signer_without_ocsp_eku`
- `validate_ocsp_response_rejects_multiple_matching_single_responses`
- `build_ocsp_request_includes_nonce_when_enabled`
- `validate_ocsp_response_rejects_missing_required_nonce`
- `validate_ocsp_response_rejects_mismatched_required_nonce`
- `validate_ocsp_response_accepts_missing_preferred_nonce`
- `ocsp_responder_urls_for_certificate_extracts_aia_ocsp_uri`

这些测试共同冻结了：

- request 构造
- responder URL 提取
- response status / cert status / validity / nonce / responder 授权 / signature 校验

来自 [ocsp.rs](../crates/rginx-app/tests/ocsp.rs)：

- `status_and_check_report_dynamic_ocsp_refresh_state`
- `invalid_dynamic_ocsp_response_is_rejected_before_cache_write`
- `expired_ocsp_cache_is_cleared_when_refresh_fails`

这些测试冻结了：

- runtime refresh
- cache write / clear
- `status` 与 `check` 输出语义

## Stage 0 最低门禁

后续进入 Stage 1 以后，每次阶段性提交至少要跑：

```bash
cargo test -p rginx-http --lib -- --test-threads=1
cargo test -p rginx --test check -- --test-threads=1
cargo test -p rginx --test ocsp -- --test-threads=1
cargo test -p rginx --test downstream_mtls -- --test-threads=1
```

在阶段收尾时，建议补跑：

```bash
./scripts/test-fast.sh
```

## 执行结论

Stage 0 已完成。

从现在开始：

`后续任何 x509-parser -> rasn 迁移，都必须证明自己没有打破这份文档里的行为基线。`
