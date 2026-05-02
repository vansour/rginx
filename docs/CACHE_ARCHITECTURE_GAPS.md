# rginx Cache Architecture Gaps

本文档记录 `rginx` 当前响应缓存相对成熟代理缓存实现仍存在的长期架构差距。

本版结论基于 `2026-04-30` 的主干代码，以及 NGINX OSS、Varnish、Apache Traffic Server、Envoy 的官方文档最新页。它只描述当前有效判断，不保留阶段性收口记录。

## 当前基线

`rginx` 当前已经具备一套可用的反向代理响应缓存，而不是“只有雏形”：

- zone 级能力已经包括 `max_size_bytes`、`inactive`、`default_ttl`、`max_entry_bytes`、`path_levels`、`loader_batch_entries`、`loader_sleep_millis`、`manager_batch_entries`、`manager_sleep_millis`、`inactive_cleanup_interval_secs`、`shared_index`
- route 级能力已经包括 `methods`、`statuses`、`ttl_by_status`、自定义 key、`cache_bypass`、`no_cache`、`stale_if_error`、`use_stale`、`background_update`、`lock_timeout`、`lock_age`、`min_uses`、`ignore_headers`、`range_requests`、`slice_size_bytes`、`convert_head`
- freshness 规则已经支持 `X-Accel-Expires`、`Cache-Control: max-age`、`Expires`、`ttl_by_status`，同时支持 `stale-if-error`、`stale-while-revalidate`、`must-revalidate`、`proxy-revalidate`
- lookup/store 主路径已经支持 fill lock、本地和共享锁等待、304 revalidate、generic `Vary`、single-range cache、slice range cache、`HEAD -> GET` 缓存填充、zone/key/prefix 级 `purge`
- `shared_index` 已经不是旧的 sidecar 文件，而是带 generation 同步的 SQLite 共享元数据存储

因此，旧版本文档里把 `background_update`、`ttl_by_status`、通用 `Vary`、`cache_bypass`、`no_cache`、loader/manager tunables、`shared_index`、`convert_head` 记为缺口的判断已经失效。

但当前实现仍然有三个非常明确的边界：

- 常规可缓存响应已经支持边向下游转发、边把 body 渐进式写入临时缓存文件；但需要下游 range trim 的路径仍会走 buffered `collect()`，而且命中对外可见仍要等对象完整提交
- `response_buffering = Off` 时，代理路径会直接 bypass cache
- `shared_index` 是共享磁盘元数据库，不是 NGINX `keys_zone` 那种 shared memory，也不是 ATS 的 RAM cache

这决定了 `rginx` 当前更接近“功能完整的第一代缓存”，还不是“面向大对象、高并发、长周期运行”的成熟缓存引擎。

## 对比结论

### 对 NGINX OSS

与 NGINX OSS 相比，`rginx` 在第一层配置面上已经不算薄弱。

- 已对齐的方向包括 `cache_bypass`、`no_cache`、`background_update`、`proxy_cache_lock` 一类的回填锁控制、`min_uses`、`ttl_by_status`、`ignore_headers`、`slice`、loader/manager 节流参数、`HEAD` 转 `GET` 的缓存复用
- 主要差距已经从“有没有这些开关”转成“底层缓存引擎是否成熟”

当前仍落后于 NGINX 的点：

- NGINX 的 `proxy_cache_path` 以 `keys_zone` 共享内存维护热索引，而 `rginx` 的 `shared_index` 仍是 SQLite 共享元数据库加本地内存索引同步
- NGINX 的 cache loader、manager、purger 是专门围绕磁盘缓存设计的后台维护模型，而 `rginx` 的 inactive cleanup 和 eviction 仍要从整个内存索引中筛选、排序、再批量删除
- `rginx` 已经具备“边转发边落盘”的基础流式写入，但还没有 read-while-fill，也还没把这条能力扩展到所有缓存分支；距离 NGINX 那种可长期承载大对象的成熟数据路径还有明显差距

结论：相对 NGINX，`rginx` 最明显的差距已不在配置面，而在数据路径和后台维护模型。

### 对 Varnish

与 Varnish 相比，`rginx` 的差距更像“缓存平台层级”的差距，而不只是几个 missing knobs。

- Varnish 具备 `grace`、`keep`、`req.grace`、`beresp.do_stream`、`beresp.uncacheable`、BAN 等完整对象生命周期控制
- `rginx` 当前虽然已经有 stale serve、background update、conditional revalidate、`purge`，但语义层次还明显更窄

当前仍落后于 Varnish 的点：

- `rginx` 已经能边从后端读取、边向客户端发送、边把对象写入临时缓存文件，但还缺少 Varnish `do_stream` 之后那种更成熟的对象生命周期配套，尤其是 read-while-fill 和更细的对象可见性控制
- 缺少 `grace + keep` 这种更细的 stale / revalidate 生命周期拆分；当前更多还是 `ttl + stale-if-error + stale-while-revalidate`
- 缺少 `beresp.uncacheable` / hit-for-miss / hit-for-pass 一类的“短期记住这个 key 不值得反复填充”的对象模型
- 缺少 BAN 这种逻辑失效层；当前 `rginx` 的 `purge` 还是直接按 zone/key/prefix 删除对象

从官方文档对 VCL 的可编程变量面可以推断，Varnish 的缓存策略表达力仍显著高于 `rginx` 当前的固定谓词 + 固定动作模型。

结论：相对 Varnish，`rginx` 最需要补的不是单个开关，而是对象生命周期模型、逻辑失效层和更强的策略表达力。

### 对 Apache Traffic Server

与 ATS 相比，`rginx` 的主要差距在大对象路径、读写并发模型和热对象层次化缓存。

- ATS 官方文档明确覆盖了 Read-While-Writer、RAM cache、范围请求缓存插件、缓存分层与持久化对象管理
- `rginx` 当前已有 fill lock、range cache、共享元数据同步，但还没有进入 ATS 这种“长期高吞吐缓存节点”的工程层级

当前仍落后于 ATS 的点：

- ATS 可以在对象写入过程中让后续请求直接读取正在填充的对象；`rginx` 当前仍是“首个请求填满对象，后续请求才能命中”
- ATS 有专门的 RAM cache 和更成熟的热对象淘汰模型；`rginx` 当前的 `shared_index` 不等同于 RAM cache，eviction 也仍是整表排序式实现
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

### 1. read-while-fill 与大对象缓存成熟度

这是当前最核心的差距。

- 现在的常规可缓存响应已经能把 upstream body 渐进式写入临时缓存文件，不再需要先把全量 body 收进内存
- 但命中仍要等对象完整提交，后续请求还不能像 ATS Read-While-Writer 那样跟读正在填充的对象
- 需要下游 range trim 的路径仍会退回 buffered `collect()`，`response_buffering = Off` 也仍会直接绕过缓存
- 这说明 `rginx` 虽然已经跨过“完全不能流式写入”的门槛，但还没有把缓存变成大对象和长响应的成熟主路径

长期目标：

- 让更多缓存分支都走统一的渐进式落盘路径，而不是保留 buffered 特例
- 支持首请求写入过程中后续请求跟读，或者至少提供更接近 read-while-fill 的能力
- 让 `response_buffering` 与缓存的关系从“关闭就完全绕过”逐步走向“按能力降级”

### 2. 增量式、低抖动的淘汰与 inactive cleanup

当前 eviction 和 inactive cleanup 都还偏“整表处理”。

- `cleanup_inactive_entries_in_zone()` 会先筛出 inactive key，再排序，再分批删
- `eviction_candidates()` 会在超限时复制并排序整个 `index.entries`
- 这类实现对 cache zone 规模越大越敏感，也更容易形成周期性后台抖动

长期目标：

- 把淘汰和 inactive cleanup 改成更接近增量维护的数据结构
- 引入更稳定的热度/时钟/分段队列模型，而不是全量排序式 LRU
- 让后台维护成本和缓存规模解耦得更彻底

### 3. 逻辑失效层与“不可缓存对象记忆”模型

当前 `rginx` 已经有 `purge`，但还没有成熟缓存平台常见的逻辑失效层。

- 当前失效模型仍以 zone/key/prefix 直接删除为主
- 对“短期不值得缓存的对象”，当前也缺少 hit-for-miss / hit-for-pass / uncacheable sentinel 一类机制
- 这会让热点不可缓存对象在某些场景下反复触发填充尝试

长期目标：

- 增加逻辑失效层，比如 tag / ban / predicate invalidation，而不只是直接删文件
- 为不可缓存对象建立短期记忆窗口，减少重复填充和无效锁竞争
- 让 purge、revalidate、stale serve、negative caching 形成更完整的对象生命周期模型

### 4. 真正的热元数据层，而不只是共享磁盘元数据库

`shared_index` 已经解决了跨 manager / reload 的共享元数据问题，但它还不是成熟缓存引擎意义上的 hot metadata layer。

- 当前共享索引是 SQLite 持久化元数据库，本地 lookup 仍依赖各进程自己的内存索引
- 这与 NGINX `keys_zone` 或 ATS RAM cache 的目标并不相同
- 当缓存规模继续扩大时，热 key 目录、admission count、fill lock 状态的维护仍然偏重

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

- 当前 `rginx` 的缓存实现默认绑定“本地文件体 + 本地元数据索引”
- 如果后续要做 RAM-only cache、远端元数据、对象存储后端、分布式一致性策略，当前结构的扩展成本会比较高

长期目标：

- 抽出稳定的 cache backend trait / interface
- 明确 metadata、body、fill lock、purge、admission 的边界
- 为本地磁盘后端之外的实现留出真实扩展点

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
