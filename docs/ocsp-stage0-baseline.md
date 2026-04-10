# OCSP Stage 0 Baseline

更新时间：`2026-04-10`

## 目标

Stage 0 的目标不是重构实现，而是冻结当前行为基线，确保后续
`x509-ocsp -> rasn-ocsp` 的重构有清晰回归标准。

这份文档回答三个问题：

1. 当前 OCSP 子系统对外承诺了什么行为
2. 这些行为目前由哪些测试覆盖
3. 后续阶段至少要跑哪些门禁，才能认为行为没有回退

## 行为基线

当前 OCSP 逻辑的最小行为承诺如下。

### request 构造

- 能从证书链构造 OCSP request
- 要求证书链至少包含 `leaf + issuer`
- request 的 `CertId` 由以下值组成：
  - `issuer_name_hash`
  - `issuer_key_hash`
  - `leaf serial`
- 当前使用 `SHA-1` 作为 request 中 cert id hash 算法

### response 解析

- 只接受 `Successful` 的 OCSP response status
- response 必须包含 `response_bytes`
- 只接受 `basic OCSP response` 类型
- 至少要能在 `responses` 里找到一个匹配当前证书 `CertId` 的 `SingleResponse`

### 时间校验

- `thisUpdate` 不能在未来
- 如果存在 `nextUpdate`，则 `nextUpdate` 不能早于当前时间
- 如果没有当前有效的匹配 `SingleResponse`，整体 response 视为无效

### stapling / cache 语义

- 无效 OCSP cache 文件不会被继续 stapling
- stale / expired OCSP cache 文件在 runtime refresh 失败后会被清空
- invalid refresh response 不会写入 cache

### runtime / admin 语义

- 动态 OCSP refresh 会在 admin/status 中反映：
  - `cache_loaded`
  - `auto_refresh_enabled`
  - `refreshes_total`
  - `failures_total`
  - `last_error`
- `rginx check` 会反映当前 TLS OCSP 诊断摘要

## 当前自动化覆盖

### 单元测试

来自 [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)：

- `validate_ocsp_response_matches_current_certificate`
  - 冻结“当前证书链生成的 response 能被接受”
- `validate_ocsp_response_rejects_expired_response`
  - 冻结“过期 response 必须拒绝”
- `load_certified_key_bundle_ignores_stale_ocsp_cache`
  - 冻结“stale cache 不会继续被 stapled”

### 集成测试

来自 [ocsp.rs](../crates/rginx-app/tests/ocsp.rs)：

- `status_and_check_report_dynamic_ocsp_refresh_state`
  - 冻结 runtime refresh 成功时的 admin/check 语义
- `invalid_dynamic_ocsp_response_is_rejected_before_cache_write`
  - 冻结 invalid refresh response 不入 cache
- `expired_ocsp_cache_is_cleared_when_refresh_fails`
  - 冻结 expired cache 在失败后被清空

## Stage 0 产出

Stage 0 的产出就是把当前行为用可审阅的方式固定下来：

- 现有 OCSP 行为已盘点
- 现有自动化覆盖点已映射
- 后续阶段必须通过的回归门禁已定义

也就是说，从现在开始：

`后续任何 OCSP 重构，都必须证明自己没有打破这份文档里的行为基线。`

## 后续阶段的最低回归门禁

进入 Stage 1 以后，每次阶段性提交至少要跑：

```bash
cargo test -p rginx-http certificates --lib -- --test-threads=1
cargo test -p rginx --test ocsp -- --test-threads=1
cargo test -p rginx --test check -- --test-threads=1
```

在完成依赖替换后，最终门禁应升级为：

```bash
./scripts/test-fast.sh
cargo test -p rginx --test ocsp -- --test-threads=1
cargo test -p rginx --test reload -- --test-threads=1
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Stage 0 的限制

Stage 0 不解决这些问题：

- `rcgen 0.14` 带来的测试 helper API 断裂
- `x509-ocsp` 与 `der/spki` 版本分裂
- 完整 OCSP response signature 验证

这些都属于后续阶段任务。

## 执行结论

Stage 0 已完成。

下一步进入：

`阶段 1：修复测试辅助层断裂`
