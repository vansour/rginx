# x509-parser -> rasn 迁移计划

更新时间：`2026-04-11`

## 当前阶段状态

| 阶段 | 状态 | 备注 |
| --- | --- | --- |
| 阶段 0：冻结行为与测试基线 | Done | 已补阶段 0 基线文档，见 [x509-parser-stage0-baseline.md](./x509-parser-stage0-baseline.md) |
| 阶段 1：抽出内部 PKI 抽象层 | Done | 已新增 `pki` 层，并将上层 `x509-parser` 直接依赖收口到 `pki` 与 `tls/ocsp` |
| 阶段 2：替换低风险只读路径 | Done | `pki/certificate.rs` 已切到 `rasn-pkix`，证书展示与 mTLS 身份提取保持现有行为 |
| 阶段 3：替换 CRL 与 AIA 路径 | Done | DER CRL 校验与 OCSP responder URL 发现已切到 `rasn-pkix` |
| 阶段 4：替换 OCSP 证书解析与 responder 授权逻辑 | Done | `ocsp/mod.rs` 已切到 `RasnCertificate` 进行 responder 证书解析与授权判断，保留签名桥接 |
| 阶段 5：替换 OCSP 签名校验桥接 | Done | OCSP response signature 与 responder issuer signature 已切到 `webpki` 验证路径 |
| 阶段 6：删除遗留依赖与收尾 | Done | 显式依赖与测试残留已清理；`Cargo.lock` 中若仍见 `x509-parser`，来源为 `rcgen` 的传递测试依赖 |

## 背景

仓库当前已经完成了一轮 `x509-ocsp -> rasn-ocsp` 的 OCSP 类型栈替换，见 [ocsp-rasn-refactor-plan.md](./ocsp-rasn-refactor-plan.md)。

但项目生产代码里仍然依赖 `x509-parser` 提供多项能力：

- 证书主体信息解析
- SAN / SKI / AKI / KU / EKU / BasicConstraints 提取
- 连接期 mTLS 客户端证书身份提取
- CRL DER 合法性校验
- OCSP responder certificate 校验与若干高层验证逻辑

如果目标是“完全用 `rasn` 系列包替换 `x509-parser`”，那么这不是单纯的 import 替换，而是一次分阶段的 PKI 解析与校验层重构。

## 当前使用点

当前生产代码里，`x509-parser` 的核心使用点主要集中在：

- [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)
- [connection.rs](../crates/rginx-http/src/server/connection.rs)
- [certificates.rs](../crates/rginx-http/src/state/tls_runtime/certificates.rs)
- [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)

按职责拆开看：

### 1. 证书巡检与运行态展示

文件：

- [certificates.rs](../crates/rginx-http/src/state/tls_runtime/certificates.rs)

当前依赖 `x509-parser` 做：

- subject / issuer / serial
- SAN DNS names
- SKI / AKI
- BasicConstraints
- KeyUsage / ExtendedKeyUsage
- validity 时间
- 证书链链接关系与诊断

### 2. 下游 mTLS 客户端身份提取

文件：

- [connection.rs](../crates/rginx-http/src/server/connection.rs)

当前依赖 `x509-parser` 做：

- 叶子证书 subject / issuer / serial
- SAN DNS names
- chain subjects

### 3. CRL DER 校验

文件：

- [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)

当前依赖 `x509-parser` 做：

- `parse_x509_crl` 验证 DER CRL 是否可解析且无 trailing data

### 4. OCSP 证书与 responder 校验

文件：

- [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)

当前依赖 `x509-parser` 做：

- leaf / issuer 证书解析
- AIA 中 responder URL 提取
- responderId 匹配
- responder cert 签发关系检查
- responder cert validity / KU / EKU 检查
- OCSP signature 验证所需桥接

## 目标

本计划的目标是：

1. 让生产代码不再依赖 `x509-parser`
2. 使用 `rasn` / `rasn-pkix` / `rasn-ocsp` 作为 ASN.1 / PKIX / OCSP 的统一类型栈
3. 在迁移过程中保持外部行为稳定
4. 先替换低风险只读路径，再替换高风险校验路径

## 非目标

本计划不包含：

- 替换 `rustls-pemfile`
- 替换 `rustls-native-certs`
- 替换测试用 `rcgen`
- 在同一阶段里重写 TLS 语义或 admin/status 输出格式
- 在没有 characterization tests 的情况下直接改写 OCSP 核心校验

## 设计原则

### 1. 先抽象，再替换

上层业务代码不应该直接依赖 `x509-parser` 或 `rasn` 的具体类型。

### 2. 先低风险只读路径，再高风险校验路径

优先替换证书展示、身份提取、CRL 校验；最后处理 OCSP responder 校验与签名验证。

### 3. 把“ASN.1 解码”和“证书语义判断”分层

`rasn` 负责解码；项目内部自己承担：

- 证书链诊断
- KU / EKU / BasicConstraints 解释
- responder 授权判断
- OCSP 时间与状态校验

### 4. 每阶段都必须有门禁

至少要求：

- `./scripts/test-fast.sh`
- `cargo test -p rginx --test check -- --test-threads=1`
- `cargo test -p rginx --test ocsp -- --test-threads=1`
- `cargo test -p rginx --test downstream_mtls -- --test-threads=1`

必要时补：

- `./scripts/run-tls-gate.sh`

## 分阶段计划

## 阶段 0：冻结行为与测试基线

### 目标

在替换任何实现前，冻结当前对外行为。

### 要做的事

- 盘点 `x509-parser` 在生产路径中的所有用途
- 为证书巡检、mTLS 身份提取、CRL 校验、OCSP 校验补 characterization tests
- 明确这些输出必须保持稳定：
  - `rginx check`
  - `rginx status`
  - admin/status 中 TLS 证书字段
  - access log 中 mTLS 字段
  - OCSP refresh / cache / error 语义

### 涉及文件

- [certificates.rs](../crates/rginx-http/src/state/tls_runtime/certificates.rs)
- [connection.rs](../crates/rginx-http/src/server/connection.rs)
- [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)
- [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)
- [ocsp.rs](../crates/rginx-app/tests/ocsp.rs)
- [downstream_mtls.rs](../crates/rginx-app/tests/downstream_mtls.rs)
- [check.rs](../crates/rginx-app/tests/check.rs)

### 完成标准

- 所有替换目标行为都有测试锚点
- 后续阶段可以用这些测试验证“只是等价替换，而非语义漂移”

### 当前执行结果

已完成。

本阶段已做：

- 盘点生产代码中 `x509-parser` 的全部职责边界
- 确认并收拢现有测试锚点：
  - 证书巡检：`inspect_certificate(...)`
  - mTLS 身份提取：`parse_tls_client_identity(...)`
  - admin/status TLS 字段输出
  - mTLS access log 输出
  - OCSP refresh / cache / error 语义
- 补充缺失的 characterization tests：
  - AIA responder URL 提取
  - DER CRL 成功 / trailing data 失败边界
  - 多证书链 mTLS 身份提取顺序与字段输出
- 新增阶段 0 基线文档：
  - [x509-parser-stage0-baseline.md](./x509-parser-stage0-baseline.md)

### 阶段结果判断

到阶段 0 为止：

- 证书展示、mTLS 身份提取、CRL DER 校验、OCSP 关键行为都已有明确测试锚点
- 后续阶段可以在不改变外部语义的前提下收敛到内部 PKI 抽象层
- 下一步可以进入：

`阶段 1：抽出内部 PKI 抽象层`

## 阶段 1：抽出内部 PKI 抽象层

### 目标

先把上层代码从 `x509-parser` 类型上解耦。

### 建议目录

- `crates/rginx-http/src/pki/mod.rs`
- `crates/rginx-http/src/pki/certificate.rs`
- `crates/rginx-http/src/pki/crl.rs`
- `crates/rginx-http/src/pki/ocsp.rs`

### 建议抽象

- `ParsedCertificate`
- `ParsedCertificateChain`
- `ParsedClientIdentity`
- `ParsedCrl`
- `ResponderCertificateInfo`

### 要做的事

- 把 `subject / issuer / serial / SAN / KU / EKU / SKI / AKI / validity` 统一映射到内部结构
- 把当前业务代码对 `X509Certificate`、`ParsedExtension`、`GeneralName` 的依赖收口到 `pki` 层
- 允许阶段 1 内部仍先走 `x509-parser` 实现

### 完成标准

- 上层代码不再直接出现 `X509Certificate<'_>` 之类的外部类型
- 替换面缩小到 `pki` 子模块内部

### 当前执行结果

已完成。

本阶段已做：

- 新增内部 PKI 目录与入口：
  - [mod.rs](../crates/rginx-http/src/pki/mod.rs)
  - [certificate.rs](../crates/rginx-http/src/pki/certificate.rs)
  - [crl.rs](../crates/rginx-http/src/pki/crl.rs)
- 将“只读证书解析”和“CRL DER 校验”从上层模块迁入 `pki`：
  - [connection.rs](../crates/rginx-http/src/server/connection.rs)
  - [certificates.rs](../crates/rginx-http/src/state/tls_runtime/certificates.rs)
  - [certificates.rs](../crates/rginx-http/src/tls/certificates.rs)
- 在 `pki` 层内部保留当前 `x509-parser` 实现，但上层模块不再直接 import：
  - `X509Certificate`
  - `ParsedExtension`
  - `GeneralName`
  - `parse_x509_crl`

### 阶段结果判断

阶段 1 完成后，生产代码中的 `x509-parser` 直接使用点已收敛到两类内部模块：

- `pki/*`
- [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)

这意味着：

- `server` / `state` / `tls/certificates` 已经不再直接依赖 `x509-parser`
- 后续阶段 2 可以只围绕 `pki/certificate.rs` 做只读路径替换
- 后续阶段 4 / 5 可以把 OCSP 作为单独战场处理，而不需要继续穿透上层模块

## 阶段 2：替换低风险只读路径

### 目标

先替换纯读取、纯展示、纯提取路径。

### 要做的事

- 用 `rasn-pkix` 重写证书解析与字段提取
- 替换 [connection.rs](../crates/rginx-http/src/server/connection.rs) 的客户端证书身份提取
- 替换 [certificates.rs](../crates/rginx-http/src/state/tls_runtime/certificates.rs) 的证书巡检与链路诊断

### 需要覆盖的字段

- subject
- issuer
- serial
- SAN DNS
- SKI / AKI
- BasicConstraints
- KeyUsage
- ExtendedKeyUsage
- notBefore / notAfter / expires_in_days

### 风险

- `rasn-pkix` 只给 ASN.1 结构，不直接给现成高层便捷方法
- 需要自己写 OID/extension 解码与字段解释逻辑

### 完成标准

- `status` / `check` / mTLS access log 输出保持稳定
- 生产代码中，常规证书展示路径不再依赖 `x509-parser`

### 当前执行结果

已完成。

本阶段已做：

- 将 [certificate.rs](../crates/rginx-http/src/pki/certificate.rs) 的只读证书解析实现从 `x509-parser` 改为 `rasn::der + rasn-pkix`
- 保持上层调用方不变，继续由：
  - [connection.rs](../crates/rginx-http/src/server/connection.rs)
  - [certificates.rs](../crates/rginx-http/src/state/tls_runtime/certificates.rs)
  通过 `pki` 层获取解析结果
- 在 `rasn-pkix` 路径下重建了这些字段提取：
  - `subject / issuer / serial`
  - SAN DNS names
  - SKI / AKI
  - BasicConstraints
  - KeyUsage
  - ExtendedKeyUsage
  - validity 时间
  - 证书链诊断

### 阶段结果判断

阶段 2 完成后：

- 证书展示与 mTLS 身份提取已不再依赖 `x509-parser`
- 生产代码里剩余的 `x509-parser` 实现点收缩为：
  - [crl.rs](../crates/rginx-http/src/pki/crl.rs)
  - [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)
- 下一步可以进入：

`阶段 3：替换 CRL 与 AIA 路径`

## 阶段 3：替换 CRL 与 AIA 路径

### 目标

把中等复杂度的 PKIX 解析从 `x509-parser` 迁到 `rasn` 栈。

### 要做的事

- 用 `rasn-pkix` 替换 [certificates.rs](../crates/rginx-http/src/tls/certificates.rs) 中的 DER CRL 校验
- 用 `rasn-pkix` 替换 [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 中的 AIA responder URL 提取

### 完成标准

- `parse_x509_crl` 不再出现在生产代码
- responder URL 发现逻辑全部基于 `rasn` 结构

### 当前执行结果

已完成。

本阶段已做：

- 将 [crl.rs](../crates/rginx-http/src/pki/crl.rs) 中的 DER CRL 校验从 `x509-parser::parse_x509_crl` 改为 `rasn::der::decode_with_remainder::<rasn_pkix::CertificateList>`
- 保持 trailing data 拒绝语义不变
- 将 [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 中的 AIA responder URL 提取从 `x509-parser` 改为 `rasn-pkix::Certificate + AuthorityInfoAccessSyntax`
- 保持 OCSP refresh / cache / admin/status 行为不变

### 阶段结果判断

阶段 3 完成后：

- 生产代码里已不再使用 `parse_x509_crl`
- 证书 AIA 中 responder URL 的发现路径已不再依赖 `x509-parser`
- 生产代码里剩余的 `x509-parser` 实现点仅剩：
  - [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs)

下一步可以进入：

`阶段 4：替换 OCSP 证书解析与 responder 授权逻辑`

## 阶段 4：替换 OCSP 证书解析与 responder 授权逻辑

### 目标

替换 OCSP 主流程里对 `x509-parser` 的证书层依赖，但先不改最终签名校验方案。

### 要做的事

- 用 `rasn-pkix` 解析 leaf / issuer / embedded responder cert
- 重写这些判断逻辑：
  - responderId `ByName` / `ByKey` 匹配
  - signer 是否由 issuer 签发
  - signer validity 是否覆盖当前时间
  - signer EKU 是否包含 `ocspSigning`
  - signer KU 是否允许 `digitalSignature`

### 风险

- 这是首次把 `x509-parser` 的高层证书语义判断完全迁出去
- 错误信息和边界行为容易发生漂移

### 完成标准

- [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 中除签名校验桥接外，不再依赖 `x509-parser`
- OCSP 相关单测与集成测试全绿

### 当前执行结果

已完成。

本阶段已做：

- 将 [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 中这些路径从 `X509Certificate` 切到 `RasnCertificate`：
  - leaf / issuer 证书解析
  - `CertId` 构造时的 issuer name / issuer key / serial 读取
  - responderId `ByName` / `ByKey` 匹配
  - delegated responder 的 issuer 关系判断
  - delegated responder 的 validity 检查
  - delegated responder 的 EKU / KU 检查
- 保留签名桥接：
  - Basic OCSP response signature 仍通过 `x509-parser` bridge 验证
  - responder certificate 的 issuer signature 仍通过 `x509-parser` bridge 验证

### 阶段结果判断

阶段 4 完成后：

- `x509-parser` 已不再承担 OCSP 的证书字段读取与 responder 授权语义
- 生产代码里的 `x509-parser` 剩余职责已经收敛为：
  - signature algorithm / signature value 桥接
  - responder cert / issuer cert 的临时 DER -> X509 转换，仅用于签名验证
- 下一步可以进入：

`阶段 5：替换 OCSP 签名校验桥接`

## 阶段 5：替换 OCSP 签名校验桥接

### 目标

完成最后也是最难的一步：把 OCSP signature verification 从 `x509-parser` 路径上移开。

### 现状

当前 [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 依赖 `x509-parser` 提供：

- `AlgorithmIdentifier` 解析桥接
- `BitString` 解析桥接
- `verify_signature` 高层辅助

### 方案方向

建议单独引入“签名验证后端适配层”，优先评估：

- 能否直接复用 `rustls` 当前 crypto provider
- 如果不能，则需要自建：
  - `AlgorithmIdentifier` 到签名算法的映射
  - DER 签名值到验证器输入的桥接

### 风险

- 这是整个迁移中最容易引入安全语义回退的部分
- 不应和其他阶段并行推进

### 完成标准

- 生产代码里不再依赖 `x509-parser::verify`
- responder signature 校验通过现有全部 OCSP 测试

### 当前执行结果

已完成。

本阶段已做：

- 为 [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 新增基于 `webpki` 的签名验证路径
- 用 `webpki::EndEntityCert::verify_signature(...)` 替换：
  - Basic OCSP response signature 验证
  - responder certificate 的 issuer signature 验证
- 用 `webpki::ALL_VERIFICATION_ALGS` 做 signature algorithm 选择
- 删除生产代码中残留的：
  - `X509AlgorithmIdentifier`
  - `X509BitString`
  - `verify_signed_data`
  - `X509Certificate` 桥接验签

### 阶段结果判断

阶段 5 完成后：

- `ocsp/mod.rs` 的生产路径已不再依赖 `x509-parser`
- `crates/rginx-http/src` 下的生产代码中，`x509-parser` 引用已全部清零
- 下一步只剩阶段 6：

`删除遗留依赖与收尾`

## 阶段 6：删除遗留依赖与收尾

### 目标

彻底移除生产路径中的 `x509-parser`。

### 要做的事

- 删除生产代码中的 `x509-parser` imports
- 清理测试中非必要的 `x509-parser` 依赖
- 检查 `rcgen` 的 `x509-parser` feature 是否仍需要保留
- 更新依赖与文档

### 完成标准

- [crates/rginx-http/Cargo.toml](../crates/rginx-http/Cargo.toml) 不再把 `x509-parser` 作为生产依赖
- [Cargo.toml](../Cargo.toml) 中若无其他用途，也可以移除 workspace 共享声明
- 全量 TLS / OCSP / mTLS 相关测试通过

### 当前执行结果

已完成。

本阶段已做：

- 删除 workspace 对 `x509-parser` 的显式共享声明：
  - [Cargo.toml](../Cargo.toml)
- 删除 crate 级显式依赖：
  - [crates/rginx-http/Cargo.toml](../crates/rginx-http/Cargo.toml)
  - [crates/rginx-app/Cargo.toml](../crates/rginx-app/Cargo.toml)
- 去掉 `rcgen` 上不再需要的 `x509-parser` feature：
  - [crates/rginx-http/Cargo.toml](../crates/rginx-http/Cargo.toml)
  - [crates/rginx-app/Cargo.toml](../crates/rginx-app/Cargo.toml)
- 清理测试层最后一个直接使用点：
  - [crates/rginx-app/tests/ocsp.rs](../crates/rginx-app/tests/ocsp.rs)

### 阶段结果判断

阶段 6 完成后：

- 仓库源代码与显式依赖声明中已不存在 `x509-parser`
- `crates/rginx-http/src` 的生产代码中 `x509-parser` 引用为零
- 当前如果在 `Cargo.lock` 中仍看到 `x509-parser`，来源是 `rcgen 0.14.7` 的传递测试依赖，而不是项目显式依赖或运行时依赖

如果后续目标升级为：

`让 Cargo.lock 中也完全没有 x509-parser`

那将不再是“清理阶段”，而是一个新的测试基础设施替换任务，需要评估是否替换 `rcgen`。

## 推荐执行顺序

推荐顺序如下：

1. 阶段 0：冻结行为
2. 阶段 1：抽 PKI 抽象层
3. 阶段 2：替换证书展示与身份提取
4. 阶段 3：替换 CRL 与 AIA
5. 阶段 4：替换 OCSP responder 证书逻辑
6. 阶段 5：替换签名校验桥接
7. 阶段 6：删依赖与收尾

不要从 [ocsp/mod.rs](../crates/rginx-http/src/tls/ocsp/mod.rs) 的签名校验部分直接开工。

## 当前建议

如果现在开始执行，建议先落地两个最小动作：

1. 建 `pki` 抽象层，把上层代码从 `x509-parser` 类型中解耦
2. 先替 [connection.rs](../crates/rginx-http/src/server/connection.rs) 和 [certificates.rs](../crates/rginx-http/src/state/tls_runtime/certificates.rs) 这两条低风险路径

这样做的收益是：

- 最快减少 `x509-parser` 直接暴露面
- 不会一开始就碰最脆弱的 OCSP 签名校验
- 后续阶段边界会明显清晰

## 风险清单

迁移过程中最需要盯住的风险是：

- 证书扩展解释不一致，导致 `status` / `check` 输出漂移
- SAN / KU / EKU / BasicConstraints 的语义判断回退
- responder cert 授权判断变弱
- OCSP signature 校验变得不完整
- 错误信息变化过大，影响现有测试与运维诊断

因此，本计划默认：

- 先保守等价替换
- 再做结构收敛
- 最后做依赖删除
