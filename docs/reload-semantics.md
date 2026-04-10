# Reload And Restart Semantics

## 目标

这份文档说明 `rginx` 当前对 `SIGHUP` 热重载和显式重启的边界。

## 热重载支持

当前 `SIGHUP` 热重载支持以下类别的变更：

- 路由、vhost、upstream 业务配置变更
- `include` 片段内容变更
- TLS 证书、TLS 相关策略、上游 TLS 策略变更
- 显式 `listeners: []` 模型下的 listener 新增与删除

其中，显式 listener 热增删有一个前提：

- 已存在 listener 的 `listen` 地址必须保持不变

也就是说：

- 可以新增一个新的显式 listener
- 可以删除一个已有的显式 listener
- 不能把一个已有 listener 的 `listen` 地址从 `A` 改成 `B` 并要求 `SIGHUP` 生效

## Restart Boundary

当前仍然属于 restart boundary 的字段：

- `listen`
- `listeners[].listen`
- `runtime.worker_threads`
- `runtime.accept_workers`

这些字段变化时，`SIGHUP` 会被拒绝，并保留当前活跃配置继续服务。

## 被移除 Listener 的 Drain 语义

当显式 listener 在热重载中被移除时，运行时行为是：

1. 新配置不再对该 listener 接受新连接
2. 旧 listener 的 accept loop 会进入停止流程
3. 已经在处理中的连接继续 drain
4. 该 listener 上的 worker 全部退出后，相关运行时句柄才被清理

这意味着：

- 已经进入中的请求应继续完成
- 新连接最终会变成不可达

## Reload Failure 语义

如果 reload 失败：

- 当前 revision 保持不变
- 当前配置继续服务
- admin / status 会记录失败次数、最近失败原因、active revision 和 rollback preservation

## Restart 语义

当需要变更 restart boundary 字段时，推荐使用显式重启：

```bash
rginx -s restart
```

当前重启语义仍然是：

- 新进程先继承可复用的监听 fd
- 新进程准备完成后向旧进程回报 ready
- 旧进程再进入优雅退出

## 关联文档

- [refactor-plan.md](./refactor-plan.md)
- [runtime-architecture.md](./runtime-architecture.md)
