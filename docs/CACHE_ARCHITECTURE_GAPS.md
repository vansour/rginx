# rginx Cache Architecture Gaps

本文档记录 `rginx` 当前响应缓存相对成熟代理缓存实现仍存在的长期架构差距。

本版结论基于 `2026-05-03` 的主干代码，以及 NGINX OSS、Varnish、Apache Traffic Server、Envoy 的官方文档最新页。它只描述当前有效判断，不保留阶段性收口记录。

## 当前基线

`rginx` 当前已经具备一套可用的反向代理响应缓存，而不是“只有雏形”：

- zone 级能力已经包括 `max_size_bytes`、`inactive`、`default_ttl`、`max_entry_bytes`、`path_levels`、`loader_batch_entries`、`loader_sleep_millis`、`manager_batch_entries`、`manager_sleep_millis`、`inactive_cleanup_interval_secs`、`shared_index`
- route 级能力已经包括 `methods`、`statuses`、`ttl_by_status`、自定义 key、`cache_bypass`、`no_cache`、`stale_if_error`、`grace`、`keep`、`pass_ttl`、`use_stale`、`background_update`、`lock_timeout`、`lock_age`、`min_uses`、`ignore_headers`、`range_requests`、`slice_size_bytes`、`convert_head`
- freshness 规则已经支持 `X-Accel-Expires`、`Cache-Control: max-age`、`Expires`、`ttl_by_status`，同时支持 `stale-if-error`、`stale-while-revalidate`、`must-revalidate`、`proxy-revalidate`
- lookup/store 主路径已经支持 fill lock、本地和共享锁等待、304 revalidate、generic `Vary`、single-range cache、slice range cache、`HEAD -> GET` 缓存填充、zone/key/prefix 级 `purge`
- `shared_index` 已经不是旧的 sidecar 文件或 SQLite 元数据库，而是带 generation、bounded operation ring 与 delta replay 能力的 per-zone SHM 共享元数据层

因此，旧版本文档里把 `background_update`、`ttl_by_status`、通用 `Vary`、`cache_bypass`、`no_cache`、loader/manager tunables、`shared_index`、`convert_head` 记为缺口的判断已经失效。

但当前实现仍然有三个非常明确的边界：

- 常规可缓存响应已经支持边向下游转发、边把 body 渐进式写入临时缓存文件；同进程和跨进程后续请求也已经能在部分条件下复用 in-flight fill，已提交对象命中也已经改成文件流式输出；response head 现在已有进程内本地热元数据层，但 cold path 仍依赖 metadata sidecar，跨进程也还没有共享 hot tier
- `response_buffering = Off` 时，代理路径会直接 bypass cache
- `shared_index` 现在是 per-zone SHM 元数据层；但它仍不是 NGINX `keys_zone` 那种直接在共享内存内维护 slab/hash/LRU 的完整热层，也不是 ATS 的 RAM cache

这决定了 `rginx` 当前更接近“功能完整的第一代缓存”，还不是“面向大对象、高并发、长周期运行”的成熟缓存引擎。

## 对比结论

### 对 NGINX OSS

与 NGINX OSS 相比，`rginx` 在第一层配置面上已经不算薄弱。

- 已对齐的方向包括 `cache_bypass`、`no_cache`、`background_update`、`proxy_cache_lock` 一类的回填锁控制、`min_uses`、`ttl_by_status`、`ignore_headers`、`slice`、loader/manager 节流参数、`HEAD` 转 `GET` 的缓存复用
- 主要差距已经从“有没有这些开关”转成“底层缓存引擎是否成熟”

当前仍落后于 NGINX 的点：

- NGINX 的 `proxy_cache_path` 以 `keys_zone` 共享内存维护热索引，而 `rginx` 的 `shared_index` 当前是 shm-backed 元数据文档加本地内存索引同步；它已经移除了 SQLite，但还不是 `keys_zone` 那种 in-place 热索引模型
- NGINX 的 cache loader、manager、purger 是专门围绕磁盘缓存设计的后台维护模型；`rginx` 虽然已经把 inactive cleanup / eviction 收敛到增量访问队列和 lazy 重排，但还没有 `keys_zone` / shared memory 级热索引与更成熟的后台维护层次
- `rginx` 已经具备“边转发边落盘”的基础流式写入，也已经有本地与跨进程的 in-flight fill 复用，已提交对象命中也已经流式化；response head 也已有本地热元数据路径，但仍缺少 shared memory 级热层，`response_buffering = Off` 仍会绕过缓存，距离 NGINX 那种可长期承载大对象的成熟数据路径还有明显差距

结论：相对 NGINX，`rginx` 最明显的差距已不在配置面，而在数据路径和后台维护模型。

### 对 Varnish

与 Varnish 相比，`rginx` 的差距更像“缓存平台层级”的差距，而不只是几个 missing knobs。

- Varnish 具备 `grace`、`keep`、`req.grace`、`beresp.do_stream`、`beresp.uncacheable`、BAN 等完整对象生命周期控制
- `rginx` 当前虽然已经有 stale serve、background update、conditional revalidate、`purge`，但语义层次还明显更窄

当前仍落后于 Varnish 的点：

- `rginx` 已经能边从后端读取、边向客户端发送、边把对象写入临时缓存文件，也已经支持局部的 in-flight fill 复用，committed hit 也已改成流式主路径；但还缺少 Varnish `do_stream` 之后那种更成熟的对象生命周期配套，尤其是更细的对象可见性控制和生命周期分层
- 已经补上 `grace + keep` 分层以及 hit-for-pass 风格的短期记忆对象，但仍缺少 `req.grace`、更宽的 hit-for-miss / negative cache 族谱和更强的可编程生命周期动作
- 已经补上 key / prefix / tag / all 级逻辑失效层，并且失效规则会通过共享索引跨 manager 同步；但仍缺少 Varnish 那种可编程 predicate BAN、`beresp.uncacheable` 以及更强的失效语言

从官方文档对 VCL 的可编程变量面可以推断，Varnish 的缓存策略表达力仍显著高于 `rginx` 当前的固定谓词 + 固定动作模型。

结论：相对 Varnish，`rginx` 最需要补的不是单个开关，而是对象生命周期模型、逻辑失效层和更强的策略表达力。

### 对 Apache Traffic Server

与 ATS 相比，`rginx` 的主要差距在大对象路径、读写并发模型和热对象层次化缓存。

- ATS 官方文档明确覆盖了 Read-While-Writer、RAM cache、范围请求缓存插件、缓存分层与持久化对象管理
- `rginx` 当前已有 fill lock、range cache、共享元数据同步，但还没有进入 ATS 这种“长期高吞吐缓存节点”的工程层级

当前仍落后于 ATS 的点：

- ATS 可以在对象写入过程中让后续请求直接读取正在填充的对象；`rginx` 当前已经有本地和跨进程的 in-flight fill 复用，committed hit 也已流式化，但这条能力还没有覆盖成缓存引擎的默认稳定主路径
- ATS 有专门的 RAM cache 和更成熟的热对象淘汰模型；`rginx` 当前的 `shared_index` 不等同于 RAM cache，现有淘汰也只是单层近似访问队列，还不是 ATS 那种分层 RAM cache / 热对象层次模型
- ATS 的 range 生态不只是“能缓存 206”，还包括把完整响应切块缓存、把多个小 range 聚合成更高效的缓存访问路径；`rginx` 当前仍以 single-range 和 bounded slice 为主

结论：相对 ATS，`rginx` 当前最大的短板是“还不能把缓存当作长期高负载、大对象代理层的核心路径”。

### 对 Envoy

与 Envoy 的内建 HTTP cache 相比，情况不是“`rginx` 全面落后”。

- Envoy 官方文档明确写到 cache filter 还不支持 vhost-specific 配置，并且不会存储 `HEAD` 请求
- `rginx` 当前已经有 route 级 `cache_bypass`、`no_cache`、`ttl_by_status`、generic `Vary`、`convert_head`、shared fill lock、共享元数据同步、zone/key/prefix `purge`

相对 Envoy，`rginx` 的主要差距反而更集中在可扩展性：

- Envoy 的 `HttpCache` 抽象可以对接 simple memory cache、filesystem cache 或自定义后端；`rginx` 当前缓存仍假定“本地文件 body + 本地元数据索引”
- Envoy 的内建 filesystem cache 是独立后端模型，替换和扩展边界更清晰；`rginx` 当前缓存实现和代理主路径耦合更深

但如果只比较“当前内建缓存能力”，`rginx` 在 route 级控制面上并不弱于 Envoy，甚至更直接。

结论：相对 Envoy，`rginx` 更需要补的是 backend 抽象和存储层可插拔性，而不是继续堆叠一层配置开关。

## 当前仍需对齐的长期差距

下面这些才是 `rginx` 缓存下一阶段真正值得投入的长期差距，按优先级排序。

### 1. 已提交对象命中路径与大对象缓存主路径统一度

这是当前最核心的差距。

- 现在的常规可缓存响应已经能把 upstream body 渐进式写入临时缓存文件，不再需要先把全量 body 收进内存
- lookup / fill 主路径也已经支持本地和跨进程的 in-flight fill 复用，而不是所有后续请求都只能阻塞等待对象完整提交
- 已提交对象命中也已经改成文件流式响应，不再每次把整个 body 一次性读入内存
- response head 现在已经有进程内本地热元数据层，命中不再总是读取 metadata sidecar；但 cold path 仍依赖 sidecar，`response_buffering = Off` 也仍会直接绕过缓存，这说明流式写入能力还没有被推广成更稳的通用缓存主路径

长期目标：

- 让 committed hit、stale hit、revalidated hit 与 in-flight fill 尽量共享统一的数据路径
- 把本地 hot metadata path 继续推进成更稳定的默认主路径，并为后续共享热层留边界
- 把现有的本地 / 跨进程 in-flight fill 复用做成默认稳定主路径，而不是局部能力
- 让 `response_buffering` 与缓存的关系从“关闭就完全绕过”逐步走向“按能力降级”

### 2. 更成熟的热度模型与后台维护层次

Phase 3 已经把 runtime eviction / inactive cleanup 改成按访问时间维护的增量队列，bootstrap load 的 `max_size_bytes` 收敛也复用了同一条 oldest-first 路径。

- 运行期维护已经不再复制或排序整张 `index.entries`，manager 的 batch / sleep 节流仍然保留
- inactive cleanup 现在会按最老候选逐个复核本地 hot access，再决定删除还是重排
- eviction 现在是近似 last-access 队列 + lazy hot rebucket，而不是精确 LRU、clock 或 shared RAM 热层

长期目标：

- 从单层近似 last-access 队列继续演进到更成熟的 clock / segmented / multi-queue 热度模型
- 继续减少单进程内存结构的字符串和重排成本，让超大 keyset 下的后台维护延迟分布更平滑
- 把热度维护继续向 shared memory / RAM tier 靠拢，而不只是在每个进程本地做增量维护

### 3. 更强的逻辑失效谓词与“不可缓存对象记忆”模型

当前 `rginx` 已经有一层可用的逻辑失效抽象，而不再只有物理 `purge`。

- 当前已经支持 zone / key / prefix / tag 级逻辑失效，失效规则写入同一份本地索引和 SHM shared index，并在 lookup 时以 lazy drop 语义删除旧对象
- 响应 tag 当前来自 `Cache-Tag`、`Surrogate-Key`、`X-Cache-Tag`，会做规范化、去重和排序
- 已经补上基于同一索引/共享索引的 hit-for-pass sentinel，热点 `no-store` / `private` / `Set-Cookie` / unsupported `Vary` 等对象不会再反复触发无意义填充
- 但更广义的 hit-for-miss / negative caching / policy-driven uncacheable taxonomy 仍未形成统一模型，也还没有 Varnish 风格的 predicate BAN / expression invalidation

长期目标：

- 把现有 key / prefix / tag / all 失效层继续扩展成更强的 predicate / BAN 语义，而不是停留在固定 selector 集合
- 把现有 hit-for-pass 扩展成更完整的短期记忆窗口，减少重复填充和无效锁竞争
- 让 purge、invalidate、revalidate、stale serve、negative caching 形成更完整的对象生命周期模型

### 4. 继续深化共享热元数据层，而不只是本地热层 + SHM 元数据文档

`shared_index` 已经解决了跨 manager / reload 的共享元数据问题，Phase 1 也已经补上了进程内本地热元数据层；但它们组合起来，仍然不是成熟缓存引擎意义上的共享 hot metadata layer。

- 当前共享索引已切到 SHM backend，覆盖 entries、admission counts、invalidations、fill locks 与 operation ring；本地 lookup 虽然已经有热 response head 和本地热访问时间，但仍依赖各进程自己的内存索引与本地热状态
- 这与 NGINX `keys_zone` 或 ATS RAM cache 的目标并不相同
- 当前 SHM 布局仍以序列化元数据文档和有界 operation ring 为核心，还不是直接在共享内存中维护 slab/hash/LRU 结构
- 当缓存规模继续扩大时，热 key 目录、admission count、fill lock 状态的维护仍需继续降低序列化和锁竞争成本
- 后续 shared memory 深化应限定在共享热元数据层；response body 继续由磁盘 `.body` 承载，`.meta.json` 继续作为重建 shared memory 的持久来源

长期目标：

- 让热元数据层更接近 shared memory 或专门的 RAM tier
- 降低 reload、跨实例同步、后台维护对磁盘元数据库的依赖
- 让热对象路径的索引查询、命中更新、admission 和锁状态维护更便宜

### 5. 更强的 range / large-object / policy 表达力

`rginx` 当前已经覆盖了主流反向代理缓存主路径，但策略面仍明显窄于 Varnish 和 ATS。

- 当前缓存写入主路径仍以 `GET` 以及 `convert_head` 下的 `HEAD` 为中心
- range cache 仍以 single-range 和 bounded slice 为主，没有更完整的 range normalization / aggregation / anti-pollution 机制
- 当前谓词模型适合常见反向代理规则，但离 VCL 或 ATS plugin 那种可编程性仍有明显距离

长期目标：

- 补齐更强的 large-object 和 range 行为控制
- 在不引入过重复杂度的前提下提高缓存策略表达力
- 让 `rginx` 的缓存从“能用”走向“可在复杂站点里精细调优”

### 6. 可插拔的 cache backend 抽象

这是相对 Envoy 更明显的一条长期差距。

- 当前已经在 `proxy/forward` 边界抽出第一层 `ForwardCacheBackend` trait，lookup、store、304 refresh、background refresh 不再直接把代理主路径硬编码到 `CacheManager`
- 但 trait 仍然停在代理边界，底层实现依然默认绑定“本地文件体 + 本地元数据索引 + 本地/共享 fill lock”
- 如果后续要做 RAM-only cache、远端元数据、对象存储后端、分布式一致性策略，metadata/body/fill lock/admission 这几层还需要继续下沉成更稳定的后端接口

长期目标：

- 把当前 proxy 边界 trait 继续下沉成稳定的 cache backend trait / interface
- 明确 metadata、body、fill lock、purge、admission 的边界
- 为本地磁盘后端之外的实现留出真实扩展点

## 下一阶段实施计划

下面这份计划不是“理想化重写”，而是按当前代码结构可渐进落地的收敛顺序。

### 设计原则

- 先优化热路径，再抽象后端；在 committed hit 仍需整对象读入内存之前，优先做 backend trait 的收益不高
- shared memory 后端已经落地，后续优先降低 SHM 元数据文档的序列化和锁竞争成本，再考虑更深的存储后端抽象
- 不追求精确 LRU；优先让命中路径便宜、后台维护低抖动，再接受近似热度模型
- 生命周期能力应统一到对象状态机，而不是继续把 stale、revalidate、negative caching 分散在多个分支条件里

### Phase 0: 命中路径流式化（已完成）

当前状态：

- committed hit 已经改成基于缓存文件的流式响应体，而不是整对象读入内存
- committed slice hit 复用了现有 downstream range trim 包装，full hit 和 range hit 现在都走文件流式输出
- 已补 committed hit / committed slice hit 的多帧回归测试，避免未来退回 `fs::read()` 式实现

已落地点：

- `crates/rginx-http/src/cache/entry.rs`
- `crates/rginx-http/src/cache/entry/response.rs`
- `crates/rginx-http/src/cache/store/range.rs`

### Phase 1: 本地热元数据层（已完成）

当前状态：

- 已引入进程内热 response head 缓存，热 key 的 hit / stale / revalidate 路径不再总是读取 metadata sidecar
- `last_access` 已从 lookup 命中时的 durable index 写触摸迁移到本地热状态；inactive cleanup 和 eviction 会优先参考本地热访问时间
- `CacheIndex` 已从单 `Mutex` 切到 `RwLock`，lookup 匹配路径不再因为命中触摸而强制持有写锁

建议切入点：

- `crates/rginx-http/src/cache/mod.rs`
- `crates/rginx-http/src/cache/lookup.rs`
- `crates/rginx-http/src/cache/index.rs`

已落地点：

- `crates/rginx-http/src/cache/mod.rs`
- `crates/rginx-http/src/cache/runtime.rs`
- `crates/rginx-http/src/cache/lookup.rs`
- `crates/rginx-http/src/cache/entry.rs`
- `crates/rginx-http/src/cache/store/maintenance/`
- `crates/rginx-http/src/cache/shared.rs`

验收状态：

- 热 key 命中时，不再总是触发 metadata 文件读取
- lookup 命中不再通过 durable index 写入更新 `last_access`
- 本地热访问时间已经纳入 inactive cleanup 和 eviction 判断

### Phase 2: 共享元数据增量同步（已完成）

当前状态：

- 已在 SHM shared index backend 上增加变更序列，`SharedIndexOperation` 会同时更新共享元数据文档与 operation ring
- 其他进程在大多数 shared index 落后场景下会直接 replay delta，而不是整份重载本地索引
- 本地热状态只会在受影响 key 或 full reload fallback 时失效，未变 key 的本地 hot metadata 不再因为常规 shared sync 被整体清空

建议切入点：

- `crates/rginx-http/src/cache/shared.rs`
- `crates/rginx-http/src/cache/shared/index_file/mod.rs`
- `crates/rginx-http/src/cache/shared/index_file/`

已落地点：

- `crates/rginx-http/src/cache/shared.rs`
- `crates/rginx-http/src/cache/shared/index_file/mod.rs`
- `crates/rginx-http/src/cache/shared/index_file/memory_backend.rs`
- `crates/rginx-http/src/cache/shared/index_file/memory_backend/`
- `crates/rginx-http/src/cache/tests/storage_p2/shared_index.rs`

验收状态：

- shared index 落后时，多数场景只做增量追平，不再整份重载
- 跨 manager 同步时，未变 key 的本地 hot response head 能跨 delta sync 保持可用
- change log 连续性异常或存储实例被重建时，会自动退回 full reload，而不是错误 replay 旧 cursor

### Phase 3: 后台维护算法重写（已完成）

当前状态：

- eviction / inactive cleanup 已切到按访问时间递增维护的增量队列，不再复制或排序整张 `index.entries`
- 本地 hot access 会在淘汰或 inactive 清理时做 lazy re-check；如果 key 已经变热，只重排该 key，不触发全表重算
- bootstrap load 的 `max_size_bytes` 收敛也复用了同一条 oldest-first 队列，shared index delta replay 进入本地 index 后也会参与同一套淘汰顺序

已落地点：

- `crates/rginx-http/src/cache/index.rs`
- `crates/rginx-http/src/cache/load.rs`
- `crates/rginx-http/src/cache/store/maintenance/index_state.rs`
- `crates/rginx-http/src/cache/store/maintenance/mod.rs`
- `crates/rginx-http/src/cache/shared/index_file/codec.rs`
- `crates/rginx-http/src/cache/shared/index_file/memory_backend/`
- `crates/rginx-http/src/cache/tests/storage_p2/shared_index.rs`
- `crates/rginx-http/src/cache/tests/storage_p3.rs`

验收状态：

- eviction 和 inactive cleanup 不再复制并排序整张 `index.entries`
- 本地 hot key 被命中后，后台维护会懒重排候选，而不是错误删除或淘汰
- loader / manager tunables 仍可用，shared delta replay 进入本地 index 后也会参与同一套淘汰顺序

### Phase 4: 生命周期模型补齐（已完成）

当前状态：

- route policy 已增加 `grace`、`keep`、`pass_ttl`，entry / shared index / metadata sidecar 也已经带上显式生命周期字段
- lookup / stale serve / background update / conditional revalidate 已统一到 `Fresh -> Grace -> Keep -> Dead` 这套对象阶段语义上
- 已引入基于同一索引路径的 hit-for-pass sentinel，热点不可缓存对象和 304 后转不可缓存对象都可以进入短期 bypass 窗口
- `stale-while-revalidate` / `stale-if-error` 的截止时间已经改成相对 expiry 推导，不再错误地相对 store time 计算

已落地点：

- `crates/rginx-core/src/config/cache.rs`
- `crates/rginx-config/src/model/cache.rs`
- `crates/rginx-config/src/compile/cache.rs`
- `crates/rginx-config/src/validate/cache.rs`
- `crates/rginx-http/src/cache/mod.rs`
- `crates/rginx-http/src/cache/index.rs`
- `crates/rginx-http/src/cache/entry.rs`
- `crates/rginx-http/src/cache/load.rs`
- `crates/rginx-http/src/cache/policy.rs`
- `crates/rginx-http/src/cache/runtime.rs`
- `crates/rginx-http/src/cache/lookup.rs`
- `crates/rginx-http/src/cache/store.rs`
- `crates/rginx-http/src/cache/store/revalidate.rs`
- `crates/rginx-http/src/cache/store/streaming.rs`
- `crates/rginx-http/src/cache/store/streaming/finalize.rs`
- `crates/rginx-http/src/cache/shared/index_file/codec.rs`
- `crates/rginx-http/src/cache/tests/storage_p4/lifecycle.rs`

验收状态：

- 热点不可缓存对象不会反复触发无意义填充
- stale、revalidate、negative cache 的行为边界可以被清晰测试和推断
- route 级配置表达力提升，但不把实现重新拉回到大量离散布尔开关

### Phase 5: 逻辑失效层与 backend 抽象（已完成）

目标：在热路径和生命周期先稳定后，再开放更强的扩展能力。

当前状态：

- 已引入基于同一索引/共享索引的逻辑失效层，当前支持 `all`、exact key、key prefix、tag 四类 selector
- 逻辑失效规则带 `created_at_unix_ms`，只会匹配 `stored_at_unix_ms <= rule.created_at_unix_ms` 的旧对象；新对象不会被旧规则持续拦截
- lookup 命中逻辑失效对象时会转成 lazy drop，再复用现有删除路径移除 index、hot head 与磁盘文件
- tag 来自 `Cache-Tag`、`Surrogate-Key`、`X-Cache-Tag` 响应头，统一做 `trim + lowercase + dedup + sort`
- 已在 `proxy/forward/cache.rs` 抽出第一层 `ForwardCacheBackend` trait；lookup、store、304 refresh 与 background refresh 已不再直接依赖 concrete `CacheManager`

建议切入点：

- `crates/rginx-http/src/cache/store.rs`
- `crates/rginx-http/src/cache/shared.rs`
- `crates/rginx-http/src/proxy/forward/cache.rs`

已落地点：

- `crates/rginx-http/src/cache/mod.rs`
- `crates/rginx-http/src/cache/invalidation.rs`
- `crates/rginx-http/src/cache/lookup.rs`
- `crates/rginx-http/src/cache/entry.rs`
- `crates/rginx-http/src/cache/load.rs`
- `crates/rginx-http/src/cache/store/helpers.rs`
- `crates/rginx-http/src/cache/store/maintenance/mod.rs`
- `crates/rginx-http/src/cache/shared.rs`
- `crates/rginx-http/src/cache/shared/index_file/`
- `crates/rginx-http/src/cache/manager/control.rs`
- `crates/rginx-http/src/state/cache.rs`
- `crates/rginx-http/src/proxy/forward/cache.rs`
- `crates/rginx-http/src/proxy/forward/success.rs`
- `crates/rginx-http/src/proxy/forward/attempt/background.rs`
- `crates/rginx-http/src/cache/tests/storage_p2/shared_index.rs`
- `crates/rginx-http/src/cache/tests/storage_p5.rs`

验收状态：

- purge 不再只有 zone/key/prefix 三级物理删除语义
- key / prefix / tag / all 失效规则已经能在本地和 shared index 间同步，并以 lazy drop 语义淘汰旧对象
- backend 第一层接口已经把代理 cache 主路径和 concrete `CacheManager` 解耦，但还没有把 trait 下沉到 metadata/body/fill lock 级别
- backend 抽象没有反过来污染 committed hit 和 fill 热路径；当前改动停留在 `proxy/forward` 边界

## 当前优先级判断

如果只排接下来的前三件事，顺序应当是：

1. 深化共享 SHM 热元数据层
2. 更强的 range / large-object / policy 表达力
3. 更成熟的 predicate invalidation / deeper backend interface

原因很直接：命中体流式化、本地热元数据层、shared metadata 增量同步、SHM shared index、Phase 3 的后台维护重写、Phase 4 的生命周期模型，以及 Phase 5 的逻辑失效层与 proxy 边界 backend trait 都已经完成。接下来最限制 cache 继续成熟的，变成了 SHM 热层内部成本、复杂 range / large-object 策略，以及“把当前第一层 invalidation / backend interface 继续向下做深”。如果这些层不继续收敛，后面的 predicate BAN、RAM tier、对象存储后端和更强策略表达力仍会耦合在当前磁盘后端细节上。

## 当前代码锚点

后续继续收敛上述差距时，优先从以下路径切入：

- `crates/rginx-core/src/config/cache.rs`
- `crates/rginx-config/src/model/cache.rs`
- `crates/rginx-http/src/cache/policy.rs`
- `crates/rginx-http/src/cache/request.rs`
- `crates/rginx-http/src/cache/lookup.rs`
- `crates/rginx-http/src/cache/store.rs`
- `crates/rginx-http/src/cache/store/maintenance/mod.rs`
- `crates/rginx-http/src/cache/store/maintenance/index_state.rs`
- `crates/rginx-http/src/cache/shared.rs`
- `crates/rginx-http/src/cache/shared/index_file/`
- `crates/rginx-http/src/cache/runtime/fill_lock.rs`
- `crates/rginx-http/src/proxy/forward/cache.rs`

## 官方对标文档

以下官方文档是本版对比的依据：

- NGINX OSS: <https://nginx.org/en/docs/http/ngx_http_proxy_module.html>
- NGINX OSS Slice: <https://nginx.org/en/docs/http/ngx_http_slice_module.html>
- Varnish Grace / Keep: <https://varnish-cache.org/docs/7.7/users-guide/vcl-grace.html>
- Varnish Purge / Ban: <https://varnish-cache.org/docs/7.7/users-guide/purging.html>
- Varnish VCL Variables: <https://varnish-cache.org/docs/7.7/reference/vcl-var.html>
- ATS Cache Basics: <https://docs.trafficserver.apache.org/en/latest/admin-guide/configuration/cache-basics.en.html>
- ATS Cache Runtime Knobs: <https://docs.trafficserver.apache.org/en/latest/admin-guide/files/records.yaml.en.html>
- ATS Range Cache Plugin: <https://docs.trafficserver.apache.org/en/latest/admin-guide/plugins/cache_range_requests.en.html>
- ATS RAM Cache: <https://docs.trafficserver.apache.org/en/latest/developer-guide/cache-architecture/ram-cache.en.html>
- Envoy Cache Filter: <https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/cache_filter>
- Envoy File System Cache: <https://www.envoyproxy.io/docs/envoy/latest/configuration/http/caches/file_system>
- Envoy File System Cache Proto: <https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/http/cache/file_system_http_cache/v3/file_system_http_cache.proto>
