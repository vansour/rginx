# rginx 缓存支持阶段计划

## 当前实施状态

- 阶段 0：已完成。第一版边界固定为 route 级反向代理响应缓存，默认关闭。
- 阶段 1：已完成。缓存配置可 parse、validate、compile，并出现在
  `rginx check` 摘要中。
- 阶段 2：已完成。已实现内存 index + 磁盘 metadata/body 的缓存模块，
  覆盖 key、TTL、存储策略、启动扫描、损坏 metadata 和对象大小限制。
- 阶段 3：已完成。代理路径已接入 lookup/store，并输出 `X-Cache:
  HIT`、`MISS`、`BYPASS`、`EXPIRED`、`STALE`。
- 阶段 4：已完成。缓存已接入 runtime snapshot/status、admin socket、
  CLI 和 access log `$cache_status`。
- 阶段 5：已完成。已支持 `ETag`/`Last-Modified` revalidation、`304`
  刷新、`REVALIDATED`、`stale-if-error`、`stale-while-revalidate`、`Vary:
  Accept-Encoding` 和 cache lock。
- 阶段 6：已完成。已接入后台 inactive cleanup、admin/CLI purge、reload
  保留 cache data 和安装脚本 cache 根目录准备。

本文档按阶段规划 rginx 的缓存支持。第一目标是实现用户可配置的 HTTP
反向代理响应缓存。现有内部缓存，例如 DNS 解析缓存、上游 client pool、
HTTP/3 session、TLS session 和 OCSP staple cache，继续作为运行时实现细节；
除非后续阶段明确扩展其配置或观测面。

## 下一阶段：对标 NGINX 的功能补齐清单

当前阶段 0 到阶段 6 已完成，rginx 已具备可工作的 route 级反向代理响应缓存。
但如果对标开源 NGINX 的 `proxy_cache` 能力，当前实现更接近“基础可用”而不是
“生产级全面替代”。下面的优先级清单用于指导下一轮补齐。

对标范围：

- 以开源 NGINX 的 `proxy_cache`、`proxy_cache_path`、`proxy_cache_revalidate`、
  `proxy_cache_lock`、`proxy_cache_use_stale`、`proxy_cache_background_update`
  和 `slice` 协作能力为基准。
- 不以 `fastcgi_cache`、`uwsgi_cache`、`scgi_cache` 这类协议族扩展为近期目标。
- 不把 DNS 解析缓存、上游 client cache、HTTP/3 session cache、TLS session
  cache、OCSP staple cache 计入本节的主对标范围；它们继续作为独立运行时能力。

优先级原则：

- `P0`：优先补齐最影响现网迁移和日常缓存正确性的能力。
- `P1`：补齐大流量和复杂场景下的控制面、范围请求和后台维护细节。
- `P2`：补齐架构级能力与长周期追平项。

### P0：先补齐现网迁移阻塞项

目标：让常见 NGINX `proxy_cache` 配置可以较低成本迁移到 rginx，并明显缩小
缓存正确性和控制面的差距。

功能：

- 条件化 `cache_bypass` / `no_cache`
  - 目标：把“是否读缓存”和“是否写缓存”从当前硬编码策略提升为 route 可配置策略。
  - 价值：支持按 header、cookie、query、method、status 等条件做命中旁路或禁止落盘。
  - 当前差距：当前主要由固定规则控制，例如 `Authorization`、`Range` 和 gRPC 请求默认 bypass。
- `cache_valid` 等价能力和 `X-Accel-Expires`
  - 目标：支持按状态码配置 TTL，并支持上游通过 `X-Accel-Expires` 显式控制 freshness。
  - 价值：让缓存时长不再只能依赖上游 `Cache-Control` / `Expires` 或 zone 默认 TTL。
  - 当前差距：当前只有“状态码是否允许缓存”，没有“不同状态缓存多久”的配置面。
- 更强的 cache key 和通用 `Vary`
  - 目标：扩展 cache key 变量空间，并支持除 `Vary: *` 外的常见 `Vary` 语义。
  - 价值：支持真实页面缓存、按语言或设备维度缓存、按上游 `Vary` 正确分桶。
  - 当前差距：当前 key 变量只覆盖 `{scheme}`、`{host}`、`{uri}`、`{method}`，`Vary`
    只支持 `Accept-Encoding`。
- `background_update` / `use_stale` / `lock_timeout` / `lock_age` / `UPDATING`
  - 目标：把过期条目的并发行为从当前“同步回源 + 其余等待”为主，升级为更细粒度的前台命中、
    后台刷新和等待超时控制。
  - 价值：明显改善过期热点对象的尾延迟、惊群控制和可观测性。
  - 当前差距：当前已有 cache lock、`stale-if-error`、`stale-while-revalidate`，但没有
    独立的后台更新路径，也没有 `UPDATING` 级别的状态暴露。

建议改动范围：

- `crates/rginx-config/src/model/cache.rs`
- `crates/rginx-config/src/validate/cache.rs`
- `crates/rginx-config/src/compile/cache.rs`
- `crates/rginx-core/src/config/cache.rs`
- `crates/rginx-http/src/cache/request.rs`
- `crates/rginx-http/src/cache/policy.rs`
- `crates/rginx-http/src/cache/lookup.rs`
- `crates/rginx-http/src/cache/store.rs`
- `crates/rginx-http/src/cache/mod.rs`
- `crates/rginx-http/src/proxy/forward/cache.rs`
- `crates/rginx-http/src/proxy/forward/success.rs`
- `crates/rginx-http/src/handler/access_log.rs`

验收标准：

- route 可以独立配置 cache 读取旁路和写入旁路条件。
- route 可以按状态码定义缓存 TTL，并支持 `X-Accel-Expires`。
- `Vary: Accept-Language`、`Vary: User-Agent` 等常见场景可以正确分桶，`Vary: *` 仍保持不缓存。
- 热点 key 过期时可选择前台返回 stale、后台刷新，并暴露 `UPDATING` 或等价状态。
- 并发同 key 过期测试中，不会出现无界等待，也不会放大 upstream 压力。

### P1：补齐大对象场景和维护控制面

目标：支持更接近 NGINX 中大型生产缓存部署的行为，特别是长尾控制、大对象和后台维护。

功能：

- `cache_min_uses` 等价能力
  - 目标：为对象建立准入门槛，避免首个偶发请求就污染缓存。
  - 价值：降低长尾 API、低复用页面对 cache zone 的挤占。
- Range / `206` / slice 缓存
  - 目标：支持范围请求和切片缓存，而不是简单 bypass `Range`。
  - 价值：补齐安装包、媒体、超大静态资源这类缓存场景。
  - 当前差距：当前 `Range` 请求默认 bypass，`206` 和 `Content-Range` 响应不缓存。
- `proxy_ignore_headers` 等价能力
  - 目标：允许在 route 级覆盖上游的部分缓存相关响应头。
  - 价值：适配缓存头不规范、历史包袱较重的上游系统。
- cache loader / manager / path tunables
  - 目标：让磁盘扫描、批次加载、后台清理和目录布局可配置。
  - 价值：改善大 cache 目录、冷启动、慢磁盘和大规模部署时的运行体验。
  - 当前差距：当前 loader、inactive cleanup 和驱逐策略基本固定，调优面较少。

建议改动范围：

- `crates/rginx-core/src/config/cache.rs`
- `crates/rginx-config/src/model/cache.rs`
- `crates/rginx-config/src/validate/cache.rs`
- `crates/rginx-config/src/compile/cache.rs`
- `crates/rginx-http/src/cache/load.rs`
- `crates/rginx-http/src/cache/entry.rs`
- `crates/rginx-http/src/cache/policy.rs`
- `crates/rginx-http/src/cache/store.rs`
- `crates/rginx-http/src/cache/store/maintenance.rs`
- `crates/rginx-runtime/src/cache.rs`

验收标准：

- 同一资源在低频单次访问下不会立即落盘，达到最小使用次数后才进入缓存。
- `Range`/`206` 场景有可预测、可测试的缓存行为；若引入 slice，则 key、metadata、purge
  和 reload 都能覆盖。
- route 可以有选择地忽略 `Cache-Control`、`Expires`、`Set-Cookie`、`Vary`
  等缓存相关头。
- 大目录冷启动时，扫描和加载行为可通过配置调优，而不是单一固定策略。

### P2：架构级追平项

目标：追平 NGINX 在架构层、共享索引和极端场景下的能力，而不是只补语义缺口。

功能：

- `keys_zone` 等价的共享索引 / 多进程协调
  - 目标：让 active cache index 不再仅依赖单进程内存结构。
  - 价值：为未来多 worker、多进程或共享本地磁盘目录打基础。
- `proxy_cache_convert_head` 和更完整的方法语义
  - 目标：进一步细化 `GET`/`HEAD` 的互通规则和 admission policy。
- 未知大小流式响应和 partial-body caching
  - 目标：评估是否在不破坏流式代理行为的前提下支持更复杂的缓存写入模型。
  - 当前差距：当前无法安全确认 body size 的响应不会写入缓存。
- 更广协议族缓存
  - 目标：仅在产品边界明确扩展时，再考虑是否引入 `fastcgi_cache`、
    `uwsgi_cache`、`scgi_cache` 这类能力。

建议改动范围：

- `crates/rginx-http/src/cache/mod.rs`
- `crates/rginx-http/src/cache/load.rs`
- `crates/rginx-http/src/cache/store.rs`
- `crates/rginx-http/src/cache/store/maintenance.rs`
- `crates/rginx-http/src/state/mod.rs`
- `crates/rginx-runtime/src/cache.rs`

验收标准：

- cache index 的生命周期和可见性不再完全绑定当前单进程内存模型。
- `HEAD` 和 `GET` 的复用规则更接近 NGINX，可通过配置控制。
- 若支持流式/分段写入，其失败恢复、purge、reload 和一致性语义必须先被定义清楚。

### 暂不优先追的能力

这些能力暂不应抢占 `P0` / `P1`：

- purge 扩展
  - 当前已支持按 zone、精确 key、prefix 清理，并已接入 admin socket 和 CLI。
- 单纯增加更多统计字段
  - 当前 cache 已进入 `status`、`snapshot`、`delta`、access log 和 `cache` CLI，
    后续优先级应让位于缓存语义和控制面补齐。
- 把内部辅助缓存统一抽象成同一套外部配置
  - DNS、upstream client、HTTP/3 session、TLS session、OCSP staple cache
    目前仍更适合作为各自子系统能力独立演进。

## 下一轮推荐交付顺序

建议下一轮按以下顺序推进：

1. `P0-1`：`cache_bypass` / `no_cache`
2. `P0-2`：按状态 TTL + `X-Accel-Expires`
3. `P0-3`：通用 `Vary` + 更强 key 模型
4. `P0-4`：后台更新、等待超时和 `UPDATING`
5. `P1-1`：`cache_min_uses`
6. `P1-2`：Range / `206` / slice
7. `P1-3`：`proxy_ignore_headers`
8. `P1-4`：loader / manager tunables

交付原则：

- 每个子项都应先补配置模型、再补运行时、最后补 admin/status 和测试。
- `P0-3` 与 `P0-4` 变更较大，建议分别拆成独立设计文档和多阶段 PR。
- `P1-2` 不建议以“先支持一部分 `206`”的方式快速落地；若要做，应直接设计成
  可长期维护的 slice 或等价分片模型。

## 阶段 0：范围与边界

目标：先确定第一版缓存边界，避免影响现有代理行为。

决策：

- 第一版只缓存反向代理响应。
- 默认关闭缓存，只有 route 显式配置后才启用。
- 初始只缓存 `GET` 和 `HEAD`。
- 默认 bypass 带 `Authorization` 的请求。
- 默认不存储带 `Set-Cookie` 的响应。
- 第一版将 `Range` 请求视为 bypass。
- 采用“内存 metadata/index + 磁盘 body”的存储方式。
- 未启用缓存的 route 行为必须保持不变。

验收标准：

- 未配置缓存策略的 route 行为与当前完全一致。
- 明确 cache key、TTL、bypass、store、stale 的第一版规则。
- 不支持或无法安全判断的场景默认 bypass 或不写入缓存。

## 阶段 1：配置模型与编译链路

目标：先让缓存配置能 parse、validate、compile，并出现在 `rginx check`
输出中；暂不接入真实代理数据路径。

改动范围：

- `crates/rginx-core`：新增运行时缓存模型，例如 `CacheZone` 和
  `RouteCachePolicy`。
- `crates/rginx-config/src/model`：新增 RON 配置结构。
- `crates/rginx-config/src/validate`：校验 zone 名称、路径、TTL、容量、
  单对象大小和 route 引用。
- `crates/rginx-config/src/compile`：将 cache zones 和 route cache policy
  编译进 `ConfigSnapshot`。
- `crates/rginx-app/src/check`：渲染 cache zone 和 route cache 摘要。

候选配置形态：

```ron
cache_zones: [
    CacheZoneConfig(
        name: "default",
        path: "/var/cache/rginx/default",
        max_size_bytes: Some(1073741824),
        inactive_secs: Some(600),
        default_ttl_secs: Some(60),
        max_entry_bytes: Some(10485760),
    ),
]
```

```ron
LocationConfig(
    matcher: Prefix("/assets/"),
    handler: Proxy(upstream: "app"),
    cache: Some(CacheRouteConfig(
        zone: "default",
        methods: ["GET", "HEAD"],
        statuses: [200, 301, 302, 404],
        key: Some("{scheme}:{host}:{uri}"),
        stale_if_error_secs: Some(60),
    )),
)
```

验收标准：

- 合法缓存配置可以 parse、validate、compile。
- route 引用不存在的 cache zone 时给出清晰错误。
- 非法路径、容量、TTL、method、status 都能被校验拦截。
- `rginx check` 能展示缓存 zone 和 route cache 策略。

## 阶段 2：缓存存储模块

目标：实现独立 `rginx-http/src/cache/` 模块，先不接入 proxy 主流程。

建议模块布局：

- `mod.rs`：模块门面和窄 public API。
- `key.rs`：cache key 模板解析与渲染。
- `policy.rs`：request/response 是否可缓存的判断。
- `metadata.rs`：status、headers、时间戳、validator、body metadata。
- `store.rs`：磁盘布局、原子写入、读取、删除和损坏处理。
- `index.rs`：内存索引、字节统计、inactive 跟踪和淘汰候选选择。
- `body.rs`：将缓存对象转换为 `HttpBody`。
- `stats.rs`：HIT、MISS、BYPASS、EXPIRED、STALE、WRITE_ERROR、EVICT 计数。

初始存储策略：

- 使用 hash 后的 cache key 作为磁盘路径。
- metadata 和 body 通过临时文件加 rename 做原子写入。
- 超过 `max_entry_bytes` 的对象不写入。
- metadata 损坏时忽略或清理，不 panic。
- 内存 index 负责快速 lookup 和容量统计。
- 启动或 reload 时扫描 metadata 重建 index，并清理无法验证的半成品文件。

验收标准：

- 单元测试覆盖 key 渲染、TTL 计算、header policy、原子写、metadata
  解码失败和单对象大小限制。
- 损坏缓存条目不会导致进程崩溃。
- 中断写入不会生成可被当作有效缓存的半成品条目。

## 阶段 3：接入代理数据路径 MVP

目标：让 route 级缓存策略对真实代理请求生效。

接入点：

- 发送 upstream 请求前执行 cache lookup。
- fresh hit 直接返回缓存响应。
- 在请求上下文中记录 miss、bypass、expired、write 结果。
- upstream 成功响应后判断是否写入缓存。
- 增加 `X-Cache: HIT`、`MISS`、`BYPASS`、`EXPIRED` 或 `STALE`。

MVP 行为：

- 缓存 `GET` 和 `HEAD`。
- 默认只存储配置允许的状态码，初始可为 `200`、`301`、`302`、`404`。
- 尊重 `Cache-Control: no-store` 和 `private`。
- 尊重 `Expires` 与 `Cache-Control: max-age`。
- 上游没有显式 freshness 时使用 route `default_ttl`。
- 过期的 `Expires` 明确表示 TTL 为 0，不回退到 `default_ttl`；上游时间戳错误会导致对应响应不写入缓存。
- 无法从 body size hint 安全确认大小的响应不写入缓存，避免为缓存目的全量读取未知大小的流式响应。
- 默认 bypass `Authorization` 请求。
- 默认不存储 `Set-Cookie` 响应。
- MVP 中 expired 先按 miss 处理，不返回 stale。

验收标准：

- 集成测试验证 cacheable `GET` 的 `MISS -> HIT`。
- `HEAD` 能复用缓存 metadata，且不返回 body。
- `Authorization`、`Set-Cookie`、`no-store`、`private`、`Range` 场景按规则
  bypass 或不写入。
- 未显式启用 stale 时，上游失败不会返回过期缓存。
- 未启用缓存的 proxy 行为保持不变。

## 阶段 4：观测与 Admin

目标：把缓存行为接入现有 runtime inspection 和 admin 体系。

改动范围：

- `SharedState` 增加 cache registry 和 stats。
- `rginx-http/src/state/snapshots` 增加 cache snapshot 结构。
- admin snapshot 增加可选 `cache` module。
- CLI 增加 cache stats 渲染，可以先做独立 `cache` 命令，也可以并入
  `status` 和 `snapshot`。
- access log 增加 `$cache_status`。

指标：

- zone 条目数。
- zone 当前字节数。
- hit、miss、bypass、expired、stale。
- write success、write error。
- eviction count。
- zone 级视图稳定后，再考虑 route 级 cache counters。

验收标准：

- `snapshot --include cache` 返回结构化 JSON。
- `status` 或独立 cache 命令能展示 zone 级缓存健康状态。
- access log 可输出 cache status。
- cache stats 更新不会造成过量 snapshot churn。

## 阶段 5：HTTP 缓存语义增强

目标：从基础响应缓存升级到更完整的 HTTP cache 语义。

功能：

- 支持基于 `ETag` 和 `Last-Modified` 的条件 revalidation。
- revalidation 时发送 `If-None-Match` 和 `If-Modified-Since`。
- 上游返回 `304 Not Modified` 时刷新 metadata 并复用缓存 body。
- 请求 `Cache-Control: no-cache` 触发 revalidation。
- 支持 `stale-if-error`。
- 支持 `stale-while-revalidate`。
- 支持 `Vary`，优先实现 `Accept-Encoding`。
- 增加 cache lock，避免同 key 并发 miss 同时打 upstream。

验收标准：

- `304` 能刷新 freshness，且无需重写 body。
- upstream timeout 或 5xx 只有在策略允许时才返回 stale。
- 并发同 key miss 测试中，只有一个请求访问 upstream。
- `Vary: Accept-Encoding` 不会返回不兼容的编码版本。

## 阶段 6：运维与生命周期

目标：让缓存具备长期运行和维护能力。

功能：

- 后台淘汰和 inactive cleanup 的长期运行策略。
- admin purge：按 zone、精确 key、prefix 清理。
- 明确 reload 时 cache zone 和 route cache policy 的行为。
- 安装脚本创建 cache 目录。
- 权限和磁盘布局问题给出清晰错误。
- 磁盘空间不足时通过淘汰或受控 bypass 降级。

验收标准：

- reload 不丢弃仍可用的缓存数据。
- purge 有集成测试覆盖。
- cache 目录权限错误清晰可诊断。
- 磁盘写入失败只降级缓存，不破坏未缓存代理流量。
- 安装流程创建预期 cache 目录，并设置合理 owner 和权限。

## 推荐交付顺序

第一轮里程碑建议覆盖阶段 1 到阶段 3：配置模型、cache store skeleton、
以及 route 级 `GET`/`HEAD` 基础 `HIT`/`MISS`/`BYPASS`。阶段 4 应紧随其后，
确保缓存行为可观测后再扩展语义。阶段 5 和阶段 6 涉及 HTTP 正确性、并发
和运维行为，建议继续拆成更小的 PR 推进。
