# Cache Shared Memory Migration Plan

本文档记录 `rginx` 响应缓存从当前 SQLite shared index 迁移到 NGINX-style shared memory 元数据层的分阶段计划。

本计划基于 `2026-05-03` 的主干代码。目标不是重写整个缓存系统，而是用 per-zone shared memory 替换当前 SQLite 共享元数据库和文件型跨进程填充锁，同时保留现有磁盘缓存对象布局。

## 迁移结论

当前缓存对象仍应继续落在磁盘：

- `.body` 保存响应体。
- `.meta.json` 保存可从磁盘恢复索引所需的持久元数据。
- `load_index_from_disk` 继续作为冷启动、崩溃恢复和 shared memory 重建的 durable source。

需要替换的是共享热元数据层：

- SQLite `.rginx-index.sqlite3`
- SQLite `entries`、`admission_counts`、`invalidations`、`changes` 表
- 基于 SQLite generation / store_epoch / change_seq 的跨 manager 同步
- 文件型 `.rginx-fill-*.lock` 和 `.rginx-fill-*.state.json` 跨进程 fill lock 协议

目标形态接近 NGINX `keys_zone`：

- 每个 cache zone 拥有一个固定容量 shared memory segment。
- shared memory 保存热索引、admission count、逻辑失效规则、变更序列和 fill lock 状态。
- response body 仍由磁盘文件承载，shared memory 不做 RAM body cache。
- loader 从磁盘 metadata 批量恢复 shared memory。
- manager 基于 shared memory 做 eviction、inactive cleanup 和失效传播。

明确不进入 shared memory 的内容：

- 已提交响应体字节仍保存在 `.body` 文件。
- 持久恢复所需的完整响应 metadata 仍保存在 `.meta.json`。
- 正在填充的临时 body 字节仍保存在临时文件。
- response head 的进程内热缓存可以继续保留在 `hot_entries`，后续是否共享化单独评估。
- route/cache policy、运行时统计和控制面配置不作为 shared memory ABI 的一部分。

## Phase 0 决策

Phase 0 已完成，范围限定为文档和设计边界固化，不改变运行时代码行为。

本阶段锁定三条决策：

- shared memory 只替换共享热元数据层，不替换磁盘 body store。
- `.meta.json` 继续作为删除 SQLite 后的持久恢复来源。
- `.rginx-index.sqlite3` 在迁移窗口内只能作为 legacy artifact 处理，不再设计为长期兼容格式。

旧 SQLite 文件处理策略：

- Phase 1 到 Phase 3 期间，SQLite backend 仍可存在，用于行为对照和回滚。
- 默认 backend 切到 shm 后，启动路径不再从 `.rginx-index.sqlite3` 恢复权威状态，而是优先 attach shm，失败时扫描 `.meta.json`。
- 旧 `.rginx-index.sqlite3` 可以被忽略；如果实现自动清理，必须只删除当前 zone path 下的精确文件名，不能递归清理。
- 不做 SQLite 到 shm 的复杂在线迁移。原因是 `.meta.json` 已经是对象级持久来源，直接扫盘重建更简单、更可验证。

## 当前实现边界

核心运行时结构在 `crates/rginx-http/src/cache/state.rs`：

- `CacheZoneRuntime::index` 是每个进程自己的本地 `CacheIndex`。
- `CacheZoneRuntime::hot_entries` 是进程内 response head 热缓存。
- `CacheZoneRuntime::shared_index_store` 当前指向 shm-backed `SharedIndexStore`，旧 SQLite backend 已移除。
- `shared_index_generation`、`shared_index_store_epoch`、`shared_index_change_seq` 用于 shm delta replay / reload 协调。
- `fill_locks` 在 Linux shm backend 上也已进入 shared memory，非 shm 回退仅保留兼容路径。

shm shared index 相关代码集中在：

- `crates/rginx-http/src/cache/shared.rs`
- `crates/rginx-http/src/cache/shared/memory.rs`
- `crates/rginx-http/src/cache/shared/index_file/mod.rs`
- `crates/rginx-http/src/cache/shared/index_file/memory_backend.rs`

共享索引调用链分布在：

- `crates/rginx-http/src/cache/shared.rs`
- `crates/rginx-http/src/cache/shared/bootstrap.rs`
- `crates/rginx-http/src/cache/shared/delta.rs`
- `crates/rginx-http/src/cache/manager/bootstrap.rs`
- `crates/rginx-http/src/cache/manager/control.rs`
- `crates/rginx-http/src/cache/manager/response.rs`
- `crates/rginx-http/src/cache/store/maintenance/`

跨进程 fill lock 当前位于：

- `crates/rginx-http/src/cache/runtime/fill_lock.rs`
- `crates/rginx-http/src/cache/fill/shared.rs`

## 目标架构

### Shared Memory Zone

每个 cache zone 建立一个 shared memory segment，例如：

- Linux `shm_open` + `mmap`
- 或 `memfd_create` + inherited fd
- 或 `/dev/shm/rginx-cache-{zone}` 命名对象

segment header 至少包含：

- magic
- ABI version
- zone name hash
- configured capacity
- generation
- store_epoch
- operation sequence
- allocator state
- hash bucket count
- feature flags

shared memory 内部记录必须使用稳定布局：

- 不能存 Rust `String`、`Vec`、`HashMap`。
- 字符串和变长字段使用 offset + len。
- entry record 使用 fixed header + variable payload。
- 所有 offset 必须相对 segment base，不能存进程地址。

### Shared Data

第一阶段 shared memory 应覆盖：

- cache key 到 `CacheIndexEntry` 的映射
- base key 到 variant key 的映射，支撑 `Vary`
- admission count
- invalidation rules
- current size bytes
- hash ref count 或等价引用信息
- access schedule / LRU 近似队列
- operation ring buffer，用于本地 mirror 增量追平
- fill lock records

### Record Inventory

SQLite 当前承载的数据应映射为以下 shared memory records。

| 当前来源 | 当前字段 | shared memory record | 说明 |
| --- | --- | --- | --- |
| `metadata` | `schema_version` | `SegmentHeader::abi_version` | 用于拒绝不兼容布局。 |
| `metadata` | `generation` | `SegmentHeader::generation` | 每次 mutation 后递增。 |
| `metadata` | `store_epoch` | `SegmentHeader::store_epoch` | segment 重建时更新，用于阻断旧 cursor。 |
| `entries` | `key` | `EntryRecord::key_ref` | offset + len，另存 key hash 用于 bucket lookup。 |
| `entries` | `entry_json.kind` | `EntryRecord::kind` | response / hit-for-pass。 |
| `entries` | `entry_json.hash` | `EntryRecord::body_hash_ref` | 指向磁盘 `.body` / `.meta.json` 的 hash。 |
| `entries` | `entry_json.base_key` | `EntryRecord::base_key_ref` | 支撑 `Vary` variant lookup。 |
| `entries` | `entry_json.stored_at_unix_ms` | `EntryRecord::stored_at_unix_ms` | 对象生命周期字段。 |
| `entries` | `entry_json.expires_at_unix_ms` | `EntryRecord::expires_at_unix_ms` | 对象生命周期字段。 |
| `entries` | `entry_json.grace_until_unix_ms` | `EntryRecord::grace_until_unix_ms` | nullable timestamp。 |
| `entries` | `entry_json.keep_until_unix_ms` | `EntryRecord::keep_until_unix_ms` | nullable timestamp。 |
| `entries` | `entry_json.stale_if_error_until_unix_ms` | `EntryRecord::stale_if_error_until_unix_ms` | nullable timestamp。 |
| `entries` | `entry_json.stale_while_revalidate_until_unix_ms` | `EntryRecord::stale_while_revalidate_until_unix_ms` | nullable timestamp。 |
| `entries` | `entry_json.requires_revalidation` | `EntryRecord::requires_revalidation` | boolean flag。 |
| `entries` | `entry_json.must_revalidate` | `EntryRecord::must_revalidate` | boolean flag。 |
| `entries` | `entry_json.body_size_bytes` | `EntryRecord::body_size_bytes` | 同步维护 zone current size。 |
| `entries` | `entry_json.last_access_unix_ms` | `EntryRecord::last_access_unix_ms` | eviction / inactive cleanup 输入。 |
| `entries` | `entry_json.vary` | `VaryRecord[]` | 每项保存 header name ref、optional value ref。 |
| `entries` | `entry_json.tags` | `TagRecord[]` | tag invalidation 使用。 |
| local derived state | `variants` | `VariantRecord` | base key 到 variant entry 的共享索引。 |
| local derived state | `hash_ref_counts` | `BodyHashRefRecord` | 防止多 key 引用同一 body hash 时误删文件。 |
| local derived state | `current_size_bytes` | `SegmentHeader::current_size_bytes` | manager 全局容量判断使用。 |
| local derived state | `access_schedule` | `AccessQueueRecord` | NGINX-style manager 后续阶段使用。 |
| `admission_counts` | `key`, `uses` | `AdmissionRecord` | `min_uses` 计数。 |
| `invalidations` | `seq`, `rule_json` | `InvalidationRecord` | selector kind、selector value ref、created_at。 |
| `changes` | `seq`, `generation`, op payload | `OperationRecord` ring | 替代 SQLite change log，ring 溢出触发 full snapshot reload。 |
| lock/state files | lock nonce、updated_at、fill state | `FillLockRecord` | 替代 `.rginx-fill-*.lock` 和 `.state.json`。 |

shared memory record 的最小字段清单如下。

`SegmentHeader`：

- `magic`
- `abi_version`
- `zone_name_hash`
- `capacity_bytes`
- `generation`
- `store_epoch`
- `operation_seq`
- `entry_count`
- `current_size_bytes`
- `hash_bucket_count`
- `operation_ring_capacity`
- `allocator_free_head`
- `flags`

`EntryRecord`：

- `key_hash`
- `key_ref`
- `body_hash_ref`
- `base_key_ref`
- `kind`
- `stored_at_unix_ms`
- `expires_at_unix_ms`
- `grace_until_unix_ms`
- `keep_until_unix_ms`
- `stale_if_error_until_unix_ms`
- `stale_while_revalidate_until_unix_ms`
- `body_size_bytes`
- `last_access_unix_ms`
- `requires_revalidation`
- `must_revalidate`
- `vary_head`
- `tag_head`
- `bucket_next`
- `access_prev`
- `access_next`

`VaryRecord`：

- `entry_ref`
- `name_ref`
- `value_ref`
- `has_value`
- `next`

`TagRecord`：

- `entry_ref`
- `tag_ref`
- `next`

`VariantRecord`：

- `base_key_hash`
- `base_key_ref`
- `entry_key_ref`
- `next`

`AdmissionRecord`：

- `key_hash`
- `key_ref`
- `uses`
- `bucket_next`

`InvalidationRecord`：

- `seq`
- `selector_kind`
- `selector_value_ref`
- `created_at_unix_ms`
- `next`

`OperationRecord`：

- `seq`
- `generation`
- `op_kind`
- `key_hash`
- `key_ref`
- `entry_ref`
- `uses`
- `rule_ref`

`FillLockRecord`：

- `key_hash`
- `key_ref`
- `owner_pid`
- `owner_generation`
- `nonce_ref`
- `share_fingerprint_hash`
- `share_fingerprint_ref`
- `body_tmp_path_ref`
- `body_path_ref`
- `acquired_at_unix_ms`
- `updated_at_unix_ms`
- `state_flags`

### Synchronization

跨进程同步不能使用普通 Rust `Mutex`。候选实现：

- process-shared pthread mutex / robust mutex
- futex-based lock
- file lock 只作为初始化协调，不作为热路径锁
- coarse lock 起步，后续再拆分 bucket lock / seqlock

MVP 优先正确性：

- 所有 shared index mutation 先用一个 segment-wide write lock。
- lookup mirror sync 使用 generation 和 operation ring。
- operation ring 溢出时执行 full snapshot reload from shm。
- shm 版本不兼容或损坏时，丢弃并从磁盘 `.meta.json` 重建。

## 分阶段计划

### Phase 0: 固化迁移边界（已完成）

状态：

- 已完成，范围限定为文档和设计边界固化，不改变运行时代码行为。

目标：

- 明确 shared memory 只替换共享热元数据层。
- 保留 `.body` 和 `.meta.json` 磁盘布局。
- 保留 `load_index_from_disk` 作为恢复入口。

工作项：

- 给缓存文档补充“shared memory 不是 RAM body cache”的约束。
- 梳理 SQLite 当前承载的数据集合，形成 shared memory record 清单。
- 明确 `.rginx-index.sqlite3` 的处理策略：迁移窗口内仅用于 SQLite backend 对照，默认切到 shm 后不再作为恢复来源，最终删除运行时代码引用；旧文件默认忽略，自动清理只能精确删除当前 zone path 下的同名文件。

验收：

- 有明确的兼容性说明。
- 有字段级 record 清单。
- 不改变运行时代码行为。

### Phase 1: 抽象 shared index 后端（已完成）

状态：

- 已完成，`SharedIndexStore` 已从 SQLite 具体类型改为 backend facade。
- SQLite 具体实现已封装为内部 `SqliteSharedIndexStore` backend。
- 外部调用方继续使用 `SharedIndexStore`、`SharedIndexOperation` 和原有 `load` / `sync_state` / `load_changes_since` / `recreate` / `apply_operations` 语义。

目标：

- 把 `SharedIndexStore` 从 SQLite 具体类型改成 shared index 后端抽象。
- 当前 SQLite 行为保持不变。

工作项：

- 引入类似 `SharedIndexBackend` 的 trait 或 enum-backed facade。
- 保留现有操作语义：
  - `load`
  - `sync_state`
  - `load_changes_since`
  - `recreate`
  - `apply_operations`
- 保留 `SharedIndexOperation` 作为 mutation API。
- 将 SQLite 实现封装为一个 backend。

验收：

- 现有 shared index 测试全部通过。
- `sync_zone_shared_index_if_needed` 和 `apply_zone_shared_index_operations_locked` 不再直接依赖 SQLite 类型。
- 没有性能或行为变化。

### Phase 2: shared memory segment MVP（已完成）

状态：

- 已完成，新增 Linux `shm_open` + `mmap` 的 isolated MVP 模块。
- 已支持 per-zone name 派生、capacity 校验、create/reset、attach、unlink、header ABI 校验和 payload 基础读写。
- 该模块尚未接入缓存主路径；当前仍只作为 Phase 3 shm backend 的基础设施。

目标：

- 建立 per-zone shared memory segment。
- 支持初始化、attach、version 校验和销毁重建。
- 暂不接入主缓存路径。

工作项：

- 新增 `cache/shared/memory.rs` 模块。
- 定义 segment header、layout、allocator 和错误类型。
- 在 `SharedMemorySegmentConfig` 中固化 `capacity_bytes`，暂不扩展用户配置面。
- 实现单进程 create / attach / basic read-write 测试。
- 实现 ABI version、capacity、zone identity 校验。

验收：

- 单进程测试能稳定创建、写入、读取、drop 后重新 attach。
- 不兼容 header 会触发安全重建。
- shared memory 容量不足有明确错误。

### Phase 3: shm shared index backend（已完成）

状态：

- 已完成，新增 shm shared index backend，并接入 Phase 1 的 `SharedIndexBackend` facade。
- 默认 backend 在 Phase 3 期间仍为 SQLite；阶段 4 完成后，Linux 默认已切到 shm，`RGINX_CACHE_SHARED_INDEX_BACKEND=sqlite` 仅作为显式回退。
- shm backend 当前使用 bounded serialized snapshot + operation log 存储在 shared memory payload 中，先保证语义等价和可回滚；后续阶段再把记录布局细化为稳定 slab/bucket 结构。
- 为避免测试污染全局环境，测试代码提供显式 backend override，并已覆盖两个 `CacheManager` 实例通过 shm backend 共享 entry、admission count、purge、tag invalidation，以及 delta replay 保留未变 key 热响应头的路径。

目标：

- 在后端抽象下新增 shared memory 实现。
- 仍保留 SQLite backend 作为对照和回滚。

工作项：

- 把 `entries`、`admission_counts`、`invalidations` 映射到 shm records。
- 实现 operation ring buffer，替代 SQLite `changes` 表。
- 实现 generation / store_epoch / change_seq 等价语义。
- 实现 full snapshot from shm，用于 ring 溢出或本地 mirror 丢失。
- 增加 feature flag 或配置开关选择 SQLite / shm backend。

验收：

- 两个 `CacheManager` 实例能通过 shm 共享 entry、admission count、purge、invalidate。
- delta replay 保持未变 key 的本地 hot response head。
- operation ring 溢出后能 full reload from shm。
- SQLite 和 shm backend 在相同测试下行为一致。

### Phase 4: 启动和恢复路径切到 shm（已完成）

状态：

- 已完成，`bootstrap_shared_index` 在 Linux 上默认优先 attach shm。
- shm 缺失、损坏或版本不兼容时，会回退到磁盘缓存文件重建。
- `.rginx-index.sqlite3` 已不再作为默认共享元数据源，只保留 `RGINX_CACHE_SHARED_INDEX_BACKEND=sqlite` 显式回退。

工作项：

- 修改 `bootstrap_shared_index`：
  - attach 现有 shm。
  - header 合法且已有 generation 时加载 shm snapshot。
  - shm 缺失、损坏或版本不兼容时，从 `.meta.json` 扫描重建。
- 保留 legacy `.rginx-index.json` 导入逻辑。
- 默认路径不再写入 `.rginx-index.sqlite3`；显式 SQLite backend 仍作为短期回退和对照实现。

验收：

- 干净启动能从磁盘恢复 shm。
- reload 能 attach 既有 shm，不需要全量扫盘。
- shm 损坏能自动重建，不影响缓存 body 文件。

### Phase 5: fill lock 迁移到 shm（已完成）

状态：

- 已完成，Linux shm backend 的默认路径已使用 shm fill lock records 协调跨 manager 填充。
- 默认 shm 路径不再生成新的 `.rginx-fill-*.lock` 和 `.rginx-fill-*.state.json` 文件。
- 显式 SQLite backend 和非 shm fallback 仍保留旧文件协议，作为短期兼容路径。
- shm fill lock 记录当前仍使用 serialized document MVP，与 Phase 3 的 shared index document 同段存储；后续阶段再迁移到稳定 slab/table 布局。

工作项：

- 在 shm 中加入 fill lock table。
- record 保存：
  - key hash
  - owner pid / owner generation
  - acquired_at / updated_at
  - fill nonce
  - serialized fill state，其中包含 share fingerprint、response metadata、temporary body path、final body path、trailers、error 和 state flags
- 改造 `fill_lock_decision`，优先通过 shm acquire / wait / read-external。
- 第一版继续轮询 shared fill lock state，后续再引入 futex wake。
- 保留 stale lock 回收逻辑。

验收：

- 跨 manager 同 key 只允许一个填充者。
- 后续请求可以等待或读取兼容的 in-flight fill。
- 填充者崩溃或 stale 后记录可回收。
- 默认 shm 路径不再生成新的 `.rginx-fill-*.lock` 和 `.state.json`。

### Phase 6: 删除 SQLite

状态：已完成。运行时 shared index 已收敛到 shm-only，SQLite 代码路径和依赖已移除。

目标：

- shared memory 成为唯一 shared index 实现。
- 从依赖和代码中彻底移除 SQLite。

工作项：

- 删除 SQLite backend 文件：
  - `shared/index_file/sqlite.rs`
  - `shared/index_file/sqlite/apply.rs`
  - `shared/index_file/sqlite/load.rs`
  - `shared/index_file/schema.rs`
- 删除 `.rginx-index.sqlite3` 相关路径和日志。
- 从 `Cargo.toml` 和 `crates/rginx-http/Cargo.toml` 移除 SQLite crate 依赖。
- 更新测试中所有 SQLite 字样和断言。
- 明确旧 `.rginx-index.sqlite3` 文件的清理策略。

验收：

- workspace 中不再存在 SQLite crate 依赖引用。
- `rg ".rginx-index.sqlite3"` 只允许出现在兼容说明或迁移测试中，最终版本应无运行时代码引用。
- shared index 全量测试在 shm backend 下通过。

### Phase 7: NGINX-style loader / manager 强化

状态：已完成（MVP 强化）。默认 shm backend 已承担跨 manager 的全局访问状态传播；本地 `CacheIndex` 仍保留为 read-through mirror，但不再只依赖进程内 hot state 决定淘汰顺序。

目标：

- 让 loader、manager 围绕 shared memory 工作，而不是围绕每个进程的本地 mirror。

工作项：

- loader 继续以 `.meta.json` / `.body` 作为 durable source，并在 shm 缺失或损坏时重建默认 shm store；扫描节奏仍由 `loader_batch_entries` / `loader_sleep` 控制。
- manager 命中成功后发布 `TouchEntry` 到 shm，其他 manager 在 lookup、store、cleanup 和控制接口前同步该全局访问时间。
- eviction 和 inactive cleanup 在同步后的 mirror 上运行，使用 shm 派生的 access schedule / LRU 近似队列，不再只受本进程 hot state 影响。
- shm header 已维护 `entry_count`、`current_size_bytes`、`generation` 和 operation seq；snapshot / 控制接口会先同步 shm 再读取本地 mirror。

验收：

- 已增加跨 manager 回归：manager B 命中 A 后，manager A 同步 shm touch，再写入 C 时保留 A、淘汰未被远端命中的 B。
- 多 manager 的 eviction / inactive 状态通过 shm touch delta 收敛；本地 hot_entries 只作为进程内 response-head 优化。
- snapshot 和控制接口通过 `snapshot_with_shared_sync` / 控制面入口先同步 shm，可反映 shm 中的全局 entry count 和 current size。

剩余后续优化归入 Phase 8：

- 将当前 serialized document MVP 进一步拆成稳定 slab/table 布局。
- 将 access schedule / eviction cursor 从“同步到本地 mirror 后执行”推进为直接在 shm 内原子维护。
- 对每次命中写 shm 的 contention 做压测，必要时改为批量 touch、采样 touch 或分 bucket lock。

### Phase 8: 性能和可观测性

状态：已完成（MVP 可观测性）。默认 shm backend 已把容量、使用量、operation ring、reload/rebuild、lock contention、stale fill lock cleanup 和 capacity rejection 暴露到 cache snapshot。

目标：

- 把 shared memory backend 从功能正确推进到可长期运行。

工作项：

- 已增加 cache snapshot metrics：
  - `shared_index_shm_capacity_bytes`
  - `shared_index_shm_used_bytes`
  - `shared_index_entry_count`
  - `shared_index_current_size_bytes`
  - `shared_index_operation_ring_capacity`
  - `shared_index_operation_ring_used`
  - `shared_index_lock_contention_total`
  - `shared_index_full_reload_total`
  - `shared_index_rebuild_total`
  - `shared_index_stale_fill_lock_cleanup_total`
  - `shared_index_capacity_rejection_total`
- shm backend 通过 nonblocking `flock` first-pass 记录 lock contention；full snapshot `load()` 记录 full reload；`recreate()` 记录 rebuild；oversized document 写入失败记录 capacity rejection；stale fill lock 清理记录 cleanup。
- 增加轻量并发 hit/touch 回归，覆盖多 manager 并发命中路径下 shm touch 可用且 metrics 可见。
- 增加 backend 级 metrics 测试，覆盖 ring usage、full reload、rebuild、lock contention 和 capacity rejection。
- 增加 stale fill lock cleanup 指标断言，保留既有 shm corrupt/rebuild、ring overflow full reload、stale fill lock reclaim 等恢复类测试。

验收：

- 并发 lookup/touch 回归通过，常规命中路径在 shm metadata touch 下保持正确。
- manager / loader 相关压力可通过现有 ignored stress suite 继续扩展运行；常规测试已覆盖 shared memory 并发命中和外部 fill streaming。
- crash / reload / capacity-full 行为现在能通过 snapshot metrics 解释：rebuild、full reload、capacity rejection、stale fill lock cleanup 都有计数。

剩余后续优化：

- 运行真实生产流量压测和火焰图，基于 `shared_index_lock_contention_total` 决定是否拆分 bucket locks、read seqlock 或 LRU lock。
- 将当前 serialized document MVP 替换为稳定 slab/table 布局后，补充 allocator-level used/free metrics。

## 关键设计决策

### 容量策略

shared memory 是固定容量，必须显式配置。容量不足时需要明确策略：

- 优先触发 eviction。
- eviction 后仍不足则拒绝写入 shared index，并让本次响应 bypass cache 或只写磁盘但不发布索引。
- 不建议静默退回 SQLite 或本地-only shared index，否则会重新引入一致性问题。

### 持久性策略

shared memory 不作为 durable store。重启后的权威恢复来源是磁盘 `.meta.json` 和 `.body`。

这意味着删除 SQLite 后会失去 durable shared index，但不会失去缓存对象。代价是冷启动可能需要 loader 扫描磁盘。

### 兼容策略

推荐迁移窗口：

- 先支持 SQLite 和 shm 双 backend。
- 默认切到 shm，SQLite 仅保留为显式 fallback 和对照 backend。
- 删除 SQLite。

如果项目希望快速删除 SQLite，也应至少先完成 Phase 1 和 Phase 3，确保后端边界清晰并有行为等价测试。

### 同步策略

MVP 使用 coarse process-shared lock 是合理选择。后续只有在压测证明 contention 明显时，才拆分更细粒度结构。

优先级：

1. 正确性。
2. 崩溃可恢复。
3. reload 兼容。
4. 性能优化。

## 测试计划

需要保留并改造的测试方向：

- `cache/tests/storage_p2/shared_index.rs`
- `cache/tests/storage_p2/shared_index/sync_regressions.rs`
- `cache/tests/storage_p2/cross_process_fill.rs`
- `cache/tests/storage_p5.rs` 中 shared invalidation 相关测试

新增测试：

- shm create / attach / version mismatch / capacity mismatch。
- 两个 manager 实例共享 entries。
- 两个 manager 实例共享 admission counts。
- purge / invalidate 跨 manager 传播。
- operation ring 溢出后 full reload from shm。
- shm 损坏后从 `.meta.json` 重建。
- fill lock owner 崩溃后的 stale lock 回收。
- 容量满时的 eviction 和 rejection。
- 并发 upsert / remove / purge stress。

建议验证命令：

- `cargo test -p rginx-http cache::tests::storage_p2`
- `cargo test -p rginx-http cache::tests::storage_p5`
- `cargo test -p rginx-http cache::tests::stress`
- 项目已有 fast / slow / cache stress 脚本若仍有效，应纳入迁移验收。

## 删除 SQLite 的完成标准

删除 SQLite 只能在以下条件全部满足后执行：

- shared memory backend 覆盖 SQLite 当前承载的所有共享元数据。
- 启动恢复不依赖 `.rginx-index.sqlite3`。
- 跨 manager entry、admission、purge、invalidate、fill lock 测试通过。
- shm ABI version 和重建路径有测试。
- SQLite crate 从 workspace dependency 中移除。
- 运行时代码中不存在 `.rginx-index.sqlite3` 写入路径。
- 文档明确说明 shared memory 是 volatile metadata，磁盘 metadata 才是持久恢复来源。
