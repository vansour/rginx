# x509-ocsp -> rasn-ocsp 重构计划

更新时间：`2026-04-10`

## 当前阶段状态

| 阶段 | 状态 | 备注 |
| --- | --- | --- |
| 阶段 0：冻结行为与测试基线 | Done | 已补充开发基线文档，见 [ocsp-stage0-baseline.md](./ocsp-stage0-baseline.md) |
| 阶段 1：修复测试辅助层断裂 | Done | 已完成 `rcgen 0.14` 测试辅助层适配；当前剩余编译问题已收敛到 `x509-ocsp / der / spki` 栈 |
| 阶段 2：抽出 OCSP 模块边界 | Done | 已新增 `tls/ocsp` 子模块，并保持 `tls.rs` 对外 API 不变；当前 `x509-ocsp` 相关错误已集中到新模块 |
| 阶段 3：request 切换到 rasn-ocsp | Done | 主流程 request 生成已改为 `rasn-ocsp + rasn::der::encode`；测试中的 request 提取也改为 `rasn::der::decode` |
| 阶段 4：response 切换到 rasn-ocsp | Done | 主流程 response 解析与 OCSP 相关测试 response 构造已切到 `rasn-ocsp` |
| 阶段 5：删除 x509-ocsp 并统一依赖栈 | Done | `x509-ocsp` 与旧 `der 0.7 / spki 0.7` 依赖已从 workspace 移除 |
| 阶段 6：全量验证 | Done | 最小回归清单已通过：`test-fast`、`ocsp`、`check`、`reload` |
| 阶段 7：语义增强 | Done | 已补主流程语义 hardening：signature、responder 授权、`producedAt`、`certStatus`、更严格匹配 |

## 背景

当前项目的 OCSP 相关逻辑主要分布在这些位置：

- [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)
- [ocsp.rs](../crates/rginx-runtime/src/ocsp.rs)
- [tls_runtime.rs](../crates/rginx-http/src/state/tls_runtime.rs)
- [ocsp.rs](../crates/rginx-app/tests/ocsp.rs)

当前依赖 `x509-ocsp = 0.2.1`。在近期依赖升级后，项目出现了两类问题：

1. `rcgen 0.14` 的测试 helper API 变化，导致多个测试构造证书的代码失效
2. `x509-ocsp` 仍停留在 `der 0.7 / spki 0.7` 生态，而当前项目已显式引入 `der 0.8 / spki 0.8`

这会带来：

- `cargo check` / `cargo clippy` 失败
- OCSP 代码和测试同时混用两套 ASN.1 / PKIX 类型系统
- 后续升级成本继续增加

本计划的目标是：

- 用 `rasn-ocsp` 重构 OCSP request / response 的类型与编解码
- 删除 `x509-ocsp`
- 在重构过程中尽量保持当前运行时行为稳定

## 当前行为边界

在进入重构前，需要把当前 OCSP 子系统“已经承诺的行为”视为基线：

- 证书链可以构造 OCSP request
- responder 返回的 OCSP response 可以被解析
- 只接受 `Successful` 的 response status
- 只接受 `basic` response type
- `SingleResponse` 必须匹配当前证书的 `CertId`
- `thisUpdate / nextUpdate` 必须覆盖当前时间
- stale / invalid cache file 不会被继续 stapling
- runtime refresh 失败时，admin/status 语义保持一致

本次重构默认不改变这些对外行为。

## 非目标

本节列举的非目标仅适用于 Stage 0~6 的依赖替换阶段，不适用于后续 Stage 7（语义增强）及以后阶段。

以下内容不应与本次“依赖替换”绑在同一阶段：

- 完整 OCSP response signature 验证
- responder certificate / issuer chain 约束增强
- nonce 支持
- multi-response 更严格筛选策略
- 更复杂的 CA / responder policy

这些属于后续“语义增强”阶段，而不是“先把 `x509-ocsp` 换掉”阶段。

## 总体策略

策略是：

1. 先冻结行为
2. 再抽模块边界
3. 再替换 request 构造
4. 再替换 response 解码
5. 最后删除 `x509-ocsp` 并统一依赖

不要把“重构”与“语义升级”搅在一起。

## 阶段 0：冻结行为与测试基线

### 目标

在不修改 OCSP 主逻辑的前提下，固定当前行为。

### 要做的事

- 盘点并跑通当前 OCSP 相关单测与集成测试
- 给现有行为补 characterization tests
- 明确哪些输出是后续必须保持不变的

### 涉及文件

- [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)
- [ocsp.rs](../crates/rginx-runtime/src/ocsp.rs)
- [ocsp.rs](../crates/rginx-app/tests/ocsp.rs)
- [check.rs](../crates/rginx-app/tests/check.rs)

### 完成标准

- OCSP 行为已被测试覆盖到 request 生成、response 匹配、过期拒绝、stale cache 清理
- 后续阶段可以用这些测试判断“重构是否只是等价替换”

## 阶段 1：修复测试辅助层断裂

### 目标

先把依赖升级带来的测试层爆炸收口，避免后续重构时噪音过大。

### 要做的事

- 适配 `rcgen 0.14`
- 清理测试 helper 中旧的 `CertifiedKey { cert, key_pair }` 构造方式
- 改掉旧的 `signed_by(public_key, cert, key_pair)` 用法

### 关键位置

- [state.rs](../crates/rginx-http/src/state.rs)
- [proxy/tests.rs](../crates/rginx-http/src/proxy/tests.rs)
- [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)
- [admin.rs](../crates/rginx-app/tests/admin.rs)
- [check.rs](../crates/rginx-app/tests/check.rs)
- [downstream_mtls.rs](../crates/rginx-app/tests/downstream_mtls.rs)
- [ocsp.rs](../crates/rginx-app/tests/ocsp.rs)
- [reload.rs](../crates/rginx-app/tests/reload.rs)

### 完成标准

- `rcgen` 相关报错消失
- 剩余问题只聚焦在 `x509-ocsp` / `der` / `spki` 栈

### 当前执行结果

已完成。

本阶段已做：

- 将测试中旧的 `CertifiedKey { cert, key_pair }` 用法切到 `signing_key`
- 将旧的 `signed_by(public_key, cert, key_pair)` 迁移到基于 `Issuer::from_params(...)` 的新签发方式
- 在需要跨函数传递 CA 的测试文件中，引入本地测试包装结构，保留 `CertificateParams`
- 为 `rcgen 0.14` 在测试依赖中显式启用 `x509-parser`

当前 `cargo check` 剩余报错已不再包含 `rcgen` API 断裂，后续工作可以专注于 Stage 2+ 的 OCSP 类型栈重构。

## 阶段 2：抽出 OCSP 模块边界

### 目标

先让 OCSP 逻辑不再耦合在 TLS 大文件里，再替换实现。

### 建议目录

- `crates/rginx-http/src/tls/ocsp/mod.rs`
- `crates/rginx-http/src/tls/ocsp/request.rs`
- `crates/rginx-http/src/tls/ocsp/response.rs`
- `crates/rginx-http/src/tls/ocsp/helpers.rs`

### 需要保持不变的公开接口

- `build_ocsp_request_for_certificate`
- `validate_ocsp_response_for_certificate`
- `ocsp_responder_urls_for_certificate`

### 关键要求

- 对外函数签名先不变
- runtime 层和 state 层不感知内部实现变化

### 完成标准

- OCSP 代码已模块化
- 仍然使用现有实现
- 所有测试保持通过

### 当前执行结果

已完成。

本阶段已做：

- 在 [tls.rs](../crates/rginx-http/src/tls.rs) 下新增 `ocsp` 子模块：
  - [mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)
- 将这些职责从 [certificates.rs](../crates/rginx-http/src/tls/certificates.rs) 中拆出：
  - responder URL 提取
  - request 构造
  - response 解码
  - 当前时间窗口校验
  - OCSP 相关单元测试
- 保持 `tls.rs` 对外 API 不变：
  - `build_ocsp_request_for_certificate`
  - `validate_ocsp_response_for_certificate`
- 更新 crate 内调用方，使其改为走新边界：
  - [state.rs](../crates/rginx-http/src/state.rs)
  - [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)

### 阶段结果判断

这一阶段的目标是“边界抽离”，不是“修掉 `x509-ocsp` 的兼容性问题”。

当前 `cargo check` 剩余错误已经集中到新的 `tls/ocsp` 模块中，说明：

- OCSP 职责已从证书材料加载逻辑中剥离
- Stage 3 / Stage 4 可以只在新模块里切换实现
- 后续不需要再在 `certificates.rs` 内部继续做大规模替换

## 阶段 3：用 rasn-ocsp 重写 request 构造

### 目标

先替掉当前手工 DER 拼 request 的实现。

### 原因

request 侧最简单、风险最低，适合先切。

### 计划

- 继续用 `x509-parser` 从 leaf/issuer 证书提取：
  - issuer name raw bytes
  - issuer public key raw bytes
  - leaf serial
- 继续用当前 `SHA-1` 计算 `issuer_name_hash` / `issuer_key_hash`
- 但 `OcspRequest / TbsRequest / Request / CertId` 改用 `rasn-ocsp`
- 最终改用 `rasn::der::encode()`

### 依赖

建议新增：

- `rasn`
- `rasn-ocsp`
- 如有需要，`rasn-pkix`

### 完成标准

- 生成的 request 能通过现有 OCSP responder 测试
- request 字节序列行为可接受，不要求和旧实现逐字节完全相同

### 当前执行结果

已完成。

本阶段已做：

- 在 [mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 中引入：
  - `rasn`
  - `rasn-ocsp`
  - `rasn-pkix`
  - `num-bigint`
- 将 OCSP request 主流程从手工 DER 拼接改为 typed ASN.1 构造：
  - `RasnOcspRequest`
  - `TbsRequest`
  - `Request`
  - `RasnCertId`
- 保持当前 request 语义不变：
  - 仍使用 `SHA-1`
  - 仍使用 `leaf + issuer` 链
  - 仍从 issuer DN / public key 和 leaf serial 构造 `CertId`
- 将 [ocsp.rs](../crates/rginx-app/tests/ocsp.rs) 里的 request 提取逻辑切换为 `rasn::der::decode`

### 阶段结果判断

当前主流程里的 OCSP request 构造已经不再依赖 `x509-ocsp`。

`cargo check` 剩余问题仍集中在 response 解析与测试 response 构造部分，说明：

- Stage 3 的变更范围是收敛的
- 下一步 Stage 4 只需继续处理 response 方向

## 阶段 4：用 rasn-ocsp 重写 response 解码

### 目标

删除主库代码里对 `x509-ocsp` 的依赖。

### 计划

- 用 `rasn::der::decode<OcspResponse>()`
- 解析 `response_status`
- 提取 basic response
- 遍历 `SingleResponse`
- 与当前证书的 `CertId` 做等价比较
- 保持当前 `thisUpdate / nextUpdate` 时间校验语义

### 注意

这一阶段只做“和当前行为等价”的替换，不额外引入新的验证要求。

### 完成标准

- 主库 OCSP 逻辑不再依赖 `x509-ocsp`
- 现有 OCSP 行为保持不变

### 当前执行结果

已完成。

本阶段已做：

- 将 [mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 中的 response 主流程改为：
  - `rasn::der::decode<RasnOcspResponse>()`
  - `rasn::der::decode<RasnBasicOcspResponse>()`
  - 直接比较 typed `RasnCertId`
- 将 `response_status / response_bytes / basic response type` 校验切到 `rasn-ocsp` / `rasn` 类型
- 将 `thisUpdate / nextUpdate` 时间窗口校验切到 `rasn` 的 `GeneralizedTime`
- 将模块内 OCSP 单测 response builder 改成 `rasn-ocsp` typed 构造
- 将 [ocsp.rs](../crates/rginx-app/tests/ocsp.rs) 的 response builder 改成 `rasn-ocsp` typed 构造
- 为 `GeneralizedTime` 构造补充 `chrono` 直依赖，避免继续依赖间接类型能力

已验证：

- `cargo check --workspace --all-targets`
- `cargo test -p rginx-http ocsp --lib`
- `cargo test -p rginx --test ocsp`

### 阶段结果判断

Stage 4 完成后，当前 response 方向的主流程和相关测试都已经不再依赖 `x509-ocsp`。

下一步 Stage 5 只需要做依赖清理和统一：

- 删除 `x509-ocsp`
- 清理不再需要的旧 `der / spki` 测试依赖
- 收口 `Cargo.toml / Cargo.lock`

## 阶段 5：删除 x509-ocsp 并统一依赖栈

### 目标

彻底移除旧的 `der 0.7 / spki 0.7` 混用问题。

### 要做的事

- 删除 `x509-ocsp`
- 统一 `der / spki` 到同一条依赖栈
- 清理只为旧测试引入的旧版本类型

### 涉及配置

- 根 [Cargo.toml](../Cargo.toml)
- [crates/rginx-http/Cargo.toml](../crates/rginx-http/Cargo.toml)
- [crates/rginx-app/Cargo.toml](../crates/rginx-app/Cargo.toml)

### 完成标准

- `cargo check --workspace --all-targets` 通过
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过

### 当前执行结果

已完成。

本阶段已做：

- 从 [crates/rginx-http/Cargo.toml](../crates/rginx-http/Cargo.toml) 删除：
  - `x509-ocsp`
  - 不再使用的直接 `der`
  - 不再使用的测试侧 `spki`
- 从 [crates/rginx-app/Cargo.toml](../crates/rginx-app/Cargo.toml) 删除：
  - `x509-ocsp`
  - 不再使用的直接 `der`
  - 不再使用的测试侧 `spki`
- 更新 `Cargo.lock`，移除旧 `x509-ocsp -> der 0.7 / spki 0.7` 依赖链
- 修掉一处阶段 4 遗留的 `clippy::clone_on_copy`

已验证：

- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

额外确认：

- `rg "x509-ocsp" Cargo.lock crates/rginx-http/Cargo.toml crates/rginx-app/Cargo.toml` 已无命中
- `cargo tree --workspace` 中不再出现 `x509-ocsp`、`der 0.7`、`spki 0.7`

### 阶段结果判断

Stage 5 完成后，OCSP 依赖替换目标已经收口：

- runtime / test 逻辑已切到 `rasn-ocsp`
- workspace 已不再混用旧 `der / spki` 栈
- 编译与 lint 门槛已经恢复

下一步进入 Stage 6，全量验证运行时回归即可。

## 阶段 6：全量验证

### 目标

验证这次重构不是“编过了”，而是真正没有破坏运行时行为。

### 最少要跑

- `./scripts/test-fast.sh`
- `cargo test -p rginx --test ocsp -- --test-threads=1`
- `cargo test -p rginx --test check -- --test-threads=1`
- `cargo test -p rginx --test reload -- --test-threads=1`

### 重点验证

- invalid response 拒绝
- expired response 清理
- stale cache 不被继续 stapling
- runtime refresh 失败后的 cache 与 admin snapshot 语义
- `rginx check` 输出不回退

### 当前执行结果

已完成。

本阶段实际执行并通过：

- `./scripts/test-fast.sh`
- `cargo test -p rginx --test ocsp -- --test-threads=1`
- `cargo test -p rginx --test check -- --test-threads=1`
- `cargo test -p rginx --test reload -- --test-threads=1`

本阶段结果说明：

- OCSP 集成测试继续覆盖并通过：
  - invalid response 拒绝
  - expired response 清理
  - stale cache 不继续 stapling
  - dynamic refresh 的 status / check 语义
- `check` 集成测试未出现输出回退或 TLS 诊断回归
- `reload` 集成测试未出现 SIGHUP / restart / TLS 变更边界回归
- 阶段 4/5 完成后的运行时行为没有在最小回归清单中暴露出新增问题

### 阶段结果判断

到 Stage 6 为止，这次 `x509-ocsp -> rasn-ocsp` 依赖替换已经完成了从：

- 行为基线冻结
- request / response 迁移
- 旧依赖删除
- 运行时回归验证

这一整条闭环。

如果下一步继续推进，就应该进入 Stage 7，只讨论语义增强，不再把它和 Stage 0~6 的依赖替换混在一起；例如 nonce / responder policy 这类能力应放在 Stage 7 处理。

## 阶段 7：语义增强

### 目标

在依赖替换完成且系统恢复稳定之后，再单独提升 OCSP 安全语义。

### 可选方向

- 完整 OCSP response signature 验证
- responder certificate / issuer chain 验证
- `producedAt` 检查
- nonce 支持
- 多 `SingleResponse` 的更严格筛选
- CA / responder 策略配置化

### 说明

这部分应单独建 issue / 文档，不应混入本次依赖替换 PR。

### 当前执行结果

已完成。

本阶段实际落地的语义增强：

- 在 [mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 中新增：
  - `BasicOcspResponse` 签名校验
  - responder certificate 授权校验
  - `producedAt` 未来时间拒绝
  - `certStatus != good` 拒绝
  - 多个匹配 `SingleResponse` 直接拒绝
- responder 授权规则收紧为：
  - 允许 issuer 直接签发的 response
  - 允许由 issuer 直接签发、且带 `id-kp-OCSPSigning` 的 delegated responder certificate
  - 若 delegated responder 带 `KeyUsage`，则要求包含 `digitalSignature`
- 响应内部 signer 解析支持：
  - `ResponderId::ByKey`
  - `ResponderId::ByName`
- 将 [ocsp.rs](../crates/rginx-app/tests/ocsp.rs) 和模块内单测的 OCSP response builder 全部切到“真实签名”构造，不再使用伪签名字节
- 为 `x509-parser` 显式启用 `verify-aws`，复用现有 `aws-lc-rs` 验签能力
- 新增证书级 OCSP 配置面：
  - `ocsp: Some(OcspConfig(...))`
  - `nonce: Disabled | Preferred | Required`
  - `responder_policy: IssuerOnly | IssuerOrDelegated`
- 将 nonce 与 responder policy 接入：
  - config model / compile / validate
  - runtime refresh request 构造
  - response 验证
  - status / check 输出
- nonce 当前语义为：
  - online refresh 时可发送 nonce
  - `Preferred` 模式下，response 可省略 nonce；若返回则必须匹配
  - `Required` 模式下，response 必须回显匹配 nonce
  - cache 文件本地重载时不再重新校验 nonce，只继续校验签名、时间窗口和 responder policy

本阶段新增并通过的关键测试覆盖：

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
- `compile_preserves_server_tls_ocsp_policy_fields`

本阶段实际验证并通过：

- `cargo test -p rginx-http ocsp --lib`
- `cargo test -p rginx --test ocsp -- --test-threads=1`
- `cargo test -p rginx --test check -- --test-threads=1`
- `cargo test -p rginx --test reload -- --test-threads=1`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

### 阶段结果判断

Stage 7 完成后，当前 OCSP 子系统已经不再只是“能解析并匹配”：

- response 必须来自可验证的 signer
- delegated responder 必须经过 issuer 约束和 EKU 限制
- `producedAt` / `certStatus` / 多响应选择都比 Stage 6 更严格

当前仍未做成可配置面的内容：

- 更细粒度的 delegated responder trust policy
- nonce / policy 的 per-responder 级别控制

这些已经不属于本轮“先把 nonce 和 responder policy 接进主链路”的范围。

## 提交切分建议

建议至少拆成这些提交：

1. `test: freeze current ocsp behavior`
2. `test: adapt rcgen 0.14 helpers`
3. `refactor: extract ocsp module behind existing api`
4. `refactor: switch ocsp request generation to rasn-ocsp`
5. `refactor: switch ocsp response parsing to rasn-ocsp`
6. `build: remove x509-ocsp and align der/spki stack`
7. `test: run full ocsp regression suite`

## 风险点

### 1. request 兼容性风险

不同 ASN.1 编码实现虽然语义等价，但 request 二进制可能与旧实现不同。

应对：

- 以 responder 接受与否为准
- 不追求字节级一致

### 2. response 匹配逻辑回归

最容易回归的是 `CertId` 等价比较和时间窗口判断。

应对：

- 保留阶段 0 的 characterization tests
- 优先做“等价语义”，不要一开始引入更多规则

### 3. 测试噪音掩盖真正回归

`rcgen` API 变化和 `x509-ocsp` 替换如果混在一起，很难定位问题。

应对：

- 阶段 1 单独完成

### 4. runtime 行为暗变

哪怕解析逻辑对了，也可能因为错误信息、cache 文件处理时机、admin snapshot 字段变化导致行为不一致。

应对：

- 把 `runtime + admin + check` 一起纳入回归验证

## 建议执行顺序

推荐实际推进顺序：

1. 修 `rcgen 0.14` 测试层
2. 抽 `tls/ocsp` 模块边界
3. 切 request 到 `rasn-ocsp`
4. 切 response 到 `rasn-ocsp`
5. 删除 `x509-ocsp`
6. 跑全量验证
7. 再评估语义增强

## 当前决策

当前决策是：

- 采用 `rasn-ocsp` 做纯 Rust 重构
- 不引入 `ocsp-stapler` 作为核心实现
- 不在本轮重构中扩张 OCSP 语义范围

这保证本次改动的目标足够收敛：

`先把依赖和类型系统统一，再谈更严格的验证模型。`
