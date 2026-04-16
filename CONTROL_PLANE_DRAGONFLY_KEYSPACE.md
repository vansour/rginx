# 控制面 Dragonfly Keyspace 约定

最后更新：`2026-04-16`

本文档定义控制面 Phase 3 的 Dragonfly keyspace 基线。目标是冻结 key 前缀和职责边界，让后续 worker、agent、SSE 和会话逻辑在同一套命名下迭代。

## 1. 总原则

- 所有 key 都必须挂在 `RGINX_CONTROL_DRAGONFLY_KEY_PREFIX` 之下
- Dragonfly 仅承担加速层职责，不承担真相源职责
- key 命名必须体现资源域和用途，避免“万能 hash”
- 队列、锁、presence、事件流必须拆开，不混用同一类 key

## 2. 当前基线

默认前缀：

- `rginx:control`

当前约定的 key 模式：

- `rginx:control:session:{session_id}`
- `rginx:control:deployment:queue:{cluster_id}`
- `rginx:control:deployment:lock:{cluster_id}`
- `rginx:control:node:presence:{node_id}`
- `rginx:control:event:stream:{stream_name}`
- `rginx:control:sse:fanout:{channel}`

## 3. 责任映射

- `session:*`
  - 用户会话或 agent 短期票据
- `deployment:queue:*`
  - 待执行 deployment task 队列
- `deployment:lock:*`
  - 集群级发布互斥锁
- `node:presence:*`
  - 节点在线态短 TTL presence
- `event:stream:*`
  - 实时事件流游标或 ring buffer
- `sse:fanout:*`
  - SSE/WebSocket 广播通道

## 4. 代码映射

当前仓库已在 [crates/rginx-control-store/src/dragonfly.rs](/root/github/rginx/crates/rginx-control-store/src/dragonfly.rs) 中提供统一 key 构造器，后续服务层和 worker 必须复用这一层，不直接手写裸字符串。

## 5. 明确限制

- 不允许把配置版本、deployment 最终结果、审计记录只写进 Dragonfly
- Dragonfly 中的数据必须允许重建、过期或回放
- 发布恢复和审计回查必须依赖 Postgres，而不是依赖缓存残留
