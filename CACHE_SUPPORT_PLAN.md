# rginx 缓存支持阶段计划

## 当前实施状态

- 阶段 0：已完成。第一版边界固定为 route 级反向代理响应缓存，默认关闭。
- 阶段 1：已完成。缓存配置可 parse、validate、compile，并出现在
  `rginx check` 摘要中。
- 阶段 2：已完成 MVP。已实现内存 index + 磁盘 metadata/body 的缓存模块，
  覆盖 key、TTL、存储策略、启动扫描、损坏 metadata 和对象大小限制。
- 阶段 3：已完成 MVP。代理路径已接入 lookup/store，并输出 `X-Cache:
  HIT`、`MISS`、`BYPASS`、`EXPIRED`。

本文档按阶段规划 rginx 的缓存支持。第一目标是实现用户可配置的 HTTP
反向代理响应缓存。现有内部缓存，例如 DNS 解析缓存、上游 client pool、
HTTP/3 session、TLS session 和 OCSP staple cache，继续作为运行时实现细节；
除非后续阶段明确扩展其配置或观测面。

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
