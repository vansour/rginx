# Development

本页面向准备继续扩展 `Rginx` 的开发者。

## 代码组织

建议先记住下面这些“改动落点”：

| 需求类型 | 主要文件 |
| --- | --- |
| 新增配置字段 | `crates/rginx-config/src/model.rs` |
| 配置约束校验 | `crates/rginx-config/src/validate.rs` |
| 默认值与编译 | `crates/rginx-config/src/compile.rs` |
| 共享运行时结构 | `crates/rginx-core/src/config.rs` |
| 请求路由与 handler | `crates/rginx-http/src/handler.rs` / `router.rs` |
| 代理行为 | `crates/rginx-http/src/proxy.rs` |
| 静态文件 | `crates/rginx-http/src/file.rs` |
| 连接与协议层 | `crates/rginx-http/src/server.rs` / `tls.rs` |
| 热重载与信号 | `crates/rginx-runtime/src/bootstrap.rs` / `reload.rs` / `shutdown.rs` |
| 指标与日志 | `crates/rginx-http/src/metrics.rs` / `crates/rginx-observability` |

## 常用命令

格式化：

```bash
cargo fmt --all
```

全量测试：

```bash
cargo test --workspace
```

仅检查配置能力：

```bash
cargo run -p rginx -- check --config configs/rginx.ron
```

## 测试结构

当前测试大致分为三层：

### 单元测试

分布在各 crate 的 `src/*.rs` 内。

典型覆盖：

- 路由匹配
- 配置编译
- 配置校验
- 指标渲染
- 限流
- peer 选择逻辑

### 集成测试

位于 `crates/rginx-app/tests/`。

典型覆盖：

- 配置检查
- failover
- `ip_hash`
- `least_conn`
- `backup`
- 静态文件
- reload
- upgrade
- HTTP/2
- vhost

### 行为文档

README 和 wiki 也要跟着特性变化更新，否则后续维护成本会迅速上升。

## 推荐的改功能顺序

如果你要新增一个能力，推荐顺序是：

1. 在 `model.rs` 增加原始配置字段
2. 在 `validate.rs` 增加语义约束
3. 在 `compile.rs` 加默认值、解析和运行时映射
4. 在 `rginx-core` 中扩展运行时结构
5. 在 `rginx-http` 或 `rginx-runtime` 中落真正行为
6. 先写单元测试，再补集成测试
7. 更新 README、ROADMAP 和 wiki

## 请求路径调试建议

遇到请求行为异常时，优先按下面的顺序查：

1. 是否选中了正确 vhost
2. 是否选中了正确 route
3. ACL 或限流是否先拦截了请求
4. handler 是否符合预期
5. upstream 是否被健康状态摘除
6. `trusted_proxies` 是否导致客户端 IP 被错误识别

## 运行时调试建议

打开更详细日志：

```bash
RUST_LOG=debug,rginx_http=trace cargo run -p rginx -- --config configs/rginx.ron
```

结合：

- `/status`
- `/metrics`
- 集成测试最小复现

通常会比直接肉眼读代码快很多。

## 当前开发边界

已完成的重点阶段：

- 多虚拟主机
- 代理行为增强
- 静态文件
- 重定向
- `ip_hash`
- `least_conn`
- `weight`
- `backup`

仍然明显缺口：

- 多 worker
- 动态配置 API
- regex 路由
- gRPC 代理
- 客户端侧超时治理
- Autoindex
- 压缩

## CI 与发版

当前 CI 关注：

- `cargo fmt --all --check`
- `cargo test --workspace --locked --quiet`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`

如果你新增功能但没有补测试或文档，后续维护会很快失控。

## 推荐阅读

- [Architecture](Architecture.md)
- [Configuration](Configuration.md)
- [Roadmap and Gaps](Roadmap-and-Gaps.md)
