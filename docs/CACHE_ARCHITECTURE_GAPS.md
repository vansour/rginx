# rginx Cache Architecture Gaps

本文档记录 `rginx` 当前响应缓存相对专业代理缓存实现仍存在的长期架构差距。

它不是一次性评审记录，而是后续缓存演进默认应对齐的长期目标。

## 当前定位

`rginx` 已经具备可用的 HTTP 响应缓存能力，包括：

- zone、TTL、`min_uses`、`stale-if-error`
- background update、304 revalidate、fill lock
- vary、single-range cache、slice range cache
- 基于共享元数据库的 `shared_index` 与增量 generation 同步
- cache stats、purge、reload 保留缓存

但当前实现更接近“功能完整的第一代缓存”，还不是“高并发、长周期、低抖动”的成熟缓存引擎。

## 已收敛项

### 已完成：去掉 zone 级全局 `io_lock`，改为更细粒度的并发控制

该项已于 2026-04-29 完成。

- 当前实现：
  - 以固定条带数的 `RwLock` 池替代 zone 级全局 `io_lock`
  - 同 hash 文件集的读路径走共享读锁，写入/重验证/删除走独占写锁
  - 删除路径改为 compare-and-remove，并且只在对应 hash 已经无索引引用时才删文件
- 收敛效果：
  - 不同 hash 文件集不再共享单个 zone 级 I/O 串行瓶颈
  - 命中、回填、重验证、清理之间的冲突面收敛到 hash/stripe 级别
  - 修复了“旧 entry 删除误删并发新写入文件”的竞态窗口

### 已完成：补齐一轮 `range` / `stale` / `revalidate` / `policy` 长尾控制项

该项已于 2026-04-29 完成第一轮收敛。

- 当前实现：
  - 区分响应 `no-cache` 与 `must-revalidate` / `proxy-revalidate`，不再把两者混成同一语义
  - 客户端显式强制 revalidate 时，不再走 stale serve 或 `UPDATING` 背景刷新
  - 请求带 `Cache-Control: no-store` 或 `If-Range` 时显式 bypass cache，避免落入不完整 range 语义
  - `use_stale` 扩展支持 `403` / `404` / `429`
- 收敛效果：
  - fresh 命中、expired stale、conditional revalidate 之间的边界更接近专业代理缓存语义
  - stale serve 不再错误覆盖客户端显式刷新意图
  - range cache 对条件化 range 请求的安全边界更明确
  - route 级 stale 控制面覆盖了更多现网常见降级状态

## 长期差距

### 1. 将缓存写入路径从“全量收集后落盘”升级为流式写入路径

当前实现会先把完整 upstream body `collect()` 到内存，再决定是否写入缓存。

- 当前问题：
  - 大对象缓存会放大内存占用
  - unknown-size / streaming body 目前不会进入缓存
  - 不适合把缓存作为大响应或长响应的核心加速路径
- 长期目标：
  - 支持渐进式写入缓存文件，而不是依赖完整 body 收集
  - 让缓存路径能够覆盖更大的响应体和更长的响应生命周期
  - 让“是否可缓存”不再强依赖完整 body 先入内存

### 2. 将淘汰与 inactive cleanup 从全量扫描改为增量、低抖动后台维护

当前淘汰和 inactive cleanup 都依赖较重的扫描、排序和批处理删除。

- 当前问题：
  - cache zone 变大后，后台维护成本会明显上升
  - 维护任务更容易形成周期性抖动
  - LRU / inactive 处理成本与缓存规模耦合过深
- 长期目标：
  - 使用更增量化的淘汰和清理模型
  - 降低大缓存下的扫描开销和后台波动
  - 让缓存维护任务在高负载下仍保持可预测的延迟特征

### 3. 补齐 range / stale / revalidate / policy 的长尾控制项

当前缓存核心链路已经可用，但策略宽度仍明显窄于成熟代理缓存。

- 当前问题：
  - cache method 仍局限于 `GET` / `HEAD`
  - range cache 仍以 single-range、bounded byte-range 为主
  - 复杂缓存策略的表达力仍弱于成熟变量系统和规则系统
- 长期目标：
  - 补齐更完整的缓存策略控制面
  - 扩大缓存适用场景，而不只覆盖当前主路径
  - 将“能缓存”推进到“能精细控制缓存行为”

### 4. 建立缓存专项 benchmark、压力测试与故障注入体系

当前缓存测试覆盖了功能正确性，但还缺少专门的性能与韧性验证体系。

- 当前问题：
  - 缺少缓存命中路径、回填路径、重验证路径的专项 benchmark
  - 缺少大 key 数、大对象、高并发、长时间运行的压力测试
  - 缺少缓存索引损坏、I/O 抖动、锁竞争、reload 并发等故障注入场景
- 长期目标：
  - 建立缓存专项 benchmark
  - 建立缓存高并发 / 长周期 soak 测试
  - 建立围绕索引、磁盘、锁和 reload 的故障注入验证

## 当前代码锚点

以下路径是当前缓存实现的主要锚点，后续做架构升级时应优先从这里切入：

- `crates/rginx-http/src/cache/mod.rs`
- `crates/rginx-http/src/cache/lookup.rs`
- `crates/rginx-http/src/cache/store.rs`
- `crates/rginx-http/src/cache/store/maintenance.rs`
- `crates/rginx-http/src/cache/store/revalidate.rs`
- `crates/rginx-http/src/cache/shared.rs`
- `crates/rginx-http/src/cache/shared/index_file.rs`
- `crates/rginx-http/src/cache/request.rs`
- `crates/rginx-config/src/model/cache.rs`
- `crates/rginx-config/src/validate/cache.rs`
