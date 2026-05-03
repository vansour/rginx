# Changelog

`rginx` 的发布说明现在采用手写维护，优先提供面向用户的完整变更摘要，而不是只列 PR 标题。

较早版本的历史说明仍可在 GitHub Releases 中查看。

Hand-written release notes are now the primary changelog source for `rginx`, so each release can
explain user-visible changes in plain language instead of only mirroring PR titles.

Older release history remains available in GitHub Releases.

## [0.1.4-rc.2] - 2026-05-03

这个预发布版本重点推进了两条主线：一条是 ACME 托管证书的端到端落地，另一条是缓存引擎成熟度的大幅提升，包括共享元数据、跨进程 in-flight fill 复用、流式数据路径、生命周期控制、逻辑失效层与验证工具链。

### 新增

- 新增端到端 ACME 托管证书能力，覆盖配置模型、校验、编译、runtime account/order/challenge/storage/scheduler，以及 HTTP-01 challenge 分发路径。现在 `rginx` 可以围绕托管身份完成证书签发、续期和运行期调度。
- 新增 ACME 观测与运维输出，包括 `check`、`status`、snapshot/runtime 状态中的 ACME 诊断和运行期信息，方便在发布前和线上排查证书托管状态。
- 缓存引擎新增跨进程 read-while-fill 能力，后续请求可以直接复用其他进程中的 in-flight fill，而不是一律等待对象完整提交后再命中。
- 缓存主路径新增更完整的流式写入与流式命中能力，包括边向下游转发边落盘、committed hit 基于文件流式响应、single-range 与 slice range 的渐进式缓存路径。
- 新增缓存生命周期模型能力，补齐 `grace`、`keep`、`pass_ttl`、hit-for-pass、stale-while-revalidate / stale-if-error 相关行为与测试覆盖，让 stale、revalidate、negative-like memory 的边界更可推断。
- 新增缓存逻辑失效层，支持 `all`、精确 key、key prefix 和 tag 四类 selector，并通过共享索引在多 manager 间同步；tag 会从 `Cache-Tag`、`Surrogate-Key` 和 `X-Cache-Tag` 规范化提取。
- 新增缓存 benchmark、stress 与故障注入工具，包括 `scripts/run-cache-benchmark.py` 和 `scripts/run-cache-stress.sh`，为后续性能基线和回归门禁提供了直接入口。

### 更新与改进

- 共享缓存索引从旧 sidecar 文件升级为 SQLite shared metadata database，并带 generation、change log 与 delta replay 机制。跨进程同步不再总是依赖整份重载，本地 hot metadata 的保留也更稳。
- 缓存 I/O 并发控制从 zone 级单锁改成更细粒度的 striped I/O locks，热点 keyset 下的并发命中与填充冲突更少。
- 缓存内部模块边界被进一步拆清，fill、maintenance、shared index、streaming store、range path 等逻辑已经分层，便于继续向可插拔 backend 演进。
- 在 `proxy/forward` 边界引入了第一层 `ForwardCacheBackend` trait，lookup、store、304 refresh 与 background refresh 不再直接硬耦合到 concrete `CacheManager`。
- 缓存验证矩阵显著扩大，新增 shared index、cross-process fill、streaming、range、termination、stress、regression、proxy stale 等多组测试，发布前对正确性和回归的覆盖面更完整。
- `check`、`status`、admin snapshot 与 runtime 状态输出得到增强，不仅新增 ACME 内容，也补充了 cache runtime snapshot、状态汇总和长期架构文档锚点。
- 仓库文档结构收口到“长期生效文档 + 版本化发布说明”的模式，新增长期缓存架构差距文档，并移除了不再适合长期维护的阶段性/示例性文档。
- 发布流程现在支持手写双语 release notes 与更完整的 changelog 归档，减少 GitHub 自动分类导致的过短、过公式化说明。

### 问题修复

- 修复并强化了多类缓存流式填充边界条件，包括 vary-sensitive 读路径、external fill 共享状态、downstream 提前断开后的后台继续填充，以及多帧文件命中的一致性。
- 修复 shared index 相关一致性问题，补强了 review 中暴露的 generation、delta replay、admission count、purge/invalidation 传播以及热 head 保留语义。
- 修复 cache tail control 与 hit-for-pass 语义上的若干边界问题，避免不可缓存对象、过期 pass marker、stale/removal 组合路径出现错误行为。
- 修复 cache benchmark 的 upstream metrics 统计问题，避免基准输出对热点路径判断产生误导。
- 修复并强化了 ACME 的临时文件创建、listener 校验、challenge 入口和托管身份相关细节，降低签发/续期路径的脆弱点。
- 修复一些 release gate 和测试层面的回归噪音，包括缓存测试构造、状态导出、模块边界与内部 trait 可见性收口，让 `clippy`、`nextest` 与发布脚本输出更稳定。

This prerelease focuses on two major tracks: shipping end-to-end ACME managed certificates and significantly maturing the cache engine with shared metadata, cross-process in-flight fill reuse, streaming data paths, lifecycle controls, logical invalidation, and stronger verification tooling.

### New

- Added end-to-end ACME managed certificate support across config model, validation, compile, runtime account/order/challenge/storage/scheduler, and HTTP-01 challenge dispatch, enabling certificate issuance, renewal, and runtime orchestration around managed identities.
- Added ACME observability and operational surfaces in `check`, `status`, and runtime snapshots so managed certificate state can be validated during release prep and in production diagnostics.
- Added cross-process read-while-fill support in the cache engine so later requests can reuse in-flight fills across processes instead of always waiting for full object commit.
- Added more complete streaming cache data paths, including progressive store while proxying downstream, file-backed streaming committed hits, and progressive cache handling for single-range and sliced-range responses.
- Added a fuller cache lifecycle model with `grace`, `keep`, `pass_ttl`, hit-for-pass, and stronger stale-while-revalidate / stale-if-error behavior, making stale serve, revalidation, and negative-like cache memory more explicit and testable.
- Added a logical cache invalidation layer with `all`, exact key, key prefix, and tag selectors, synchronized through the shared index across managers; tags are normalized from `Cache-Tag`, `Surrogate-Key`, and `X-Cache-Tag`.
- Added cache benchmark, stress, and fault-injection tooling, including `scripts/run-cache-benchmark.py` and `scripts/run-cache-stress.sh`, to provide direct performance baselines and regression gates.

### Update & Improvement

- Upgraded the shared cache index from the old sidecar file to a SQLite shared metadata database with generation, change-log, and delta replay support, reducing full reload dependence and preserving local hot metadata more reliably.
- Replaced the zone-wide cache I/O lock with finer-grained striped I/O locks to reduce contention for hot keysets under concurrent hits and fills.
- Further modularized cache internals across fill, maintenance, shared index, streaming store, and range paths, creating a cleaner foundation for future pluggable backends.
- Introduced a first-layer `ForwardCacheBackend` trait at the `proxy/forward` boundary so lookup, store, 304 refresh, and background refresh are no longer hard-wired to the concrete `CacheManager`.
- Expanded the cache verification matrix with new shared index, cross-process fill, streaming, range, termination, stress, regression, and proxy stale test groups, substantially broadening pre-release correctness coverage.
- Improved `check`, `status`, admin snapshot, and runtime status outputs with new ACME data, richer cache runtime snapshots, clearer summary output, and better links to long-lived architecture guidance.
- Tightened repository documentation around “long-lived docs + versioned release notes,” adding a long-term cache architecture gaps document and removing docs that no longer matched the long-lived maintenance model.
- The release process now supports hand-written bilingual release notes and fuller changelog archiving, avoiding the short and formulaic style produced by pure GitHub auto-generated notes.

### Bug Fixed

- Fixed and hardened multiple streaming cache fill edge cases, including vary-sensitive read paths, external shared fill state, continued fill after downstream disconnects, and consistent multi-frame file-backed hits.
- Fixed shared index consistency issues and tightened generation, delta replay, admission count, purge/invalidation propagation, and hot-head preservation semantics exposed during review.
- Fixed several cache tail-control and hit-for-pass edge cases to avoid incorrect behavior for uncacheable objects, expired pass markers, and stale/removal interaction paths.
- Fixed incorrect upstream metrics accounting in the cache benchmark tooling to keep performance analysis aligned with actual hot-path behavior.
- Fixed and hardened ACME temporary file creation, listener validation, challenge routing, and managed-identity details to reduce fragility in issuance and renewal flows.
- Fixed several release-gate and test-layer regressions across cache test fixtures, exported state types, module boundaries, and internal trait visibility, making `clippy`, `nextest`, and release prep output more stable.
