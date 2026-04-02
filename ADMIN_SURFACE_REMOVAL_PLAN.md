# HTTP 管理面删减计划

本文档记录将 `rginx` 从“带 HTTP 管理面能力的反向代理”收口为“纯入口反向代理”的阶段性计划。

目标边界：

- 保留：文件配置、`check`、`reload`、日志、健康检查、负载均衡、限流、ACL、TLS、gRPC/grpc-web。
- 删除：HTTP `Status`、HTTP `Metrics`、HTTP `Config` 三类 handler。
- 运维方式收口为：改配置文件、执行 `check`、发送 reload 信号、查看服务器本机日志与本机状态信息。

## 阶段 0

先冻结产品边界，避免边删边摇摆。

- 明确新边界：`rginx` 只做入口反向代理，不再提供 HTTP 管理面。
- 明确删除项：`Status`、`Metrics`、`Config` 三类 handler 全部移除。
- 明确保留项：文件配置、`check`、`reload`、日志、健康检查、负载均衡、限流、ACL、TLS、gRPC/grpc-web。
- 明确替代项：状态查看只允许服务器本地进行，不再经由公网 HTTP 路由。
- 先更新产品文档中的目标说明，文件主要是 `README.md` 和 `ROADMAP.md`。

## 阶段 1

先做最小破坏的“产品定义收口”，把管理面从文档和默认配置里移除，但暂时不动太多内部实现。

- 从默认配置里删除 `/status`、`/metrics` 和任何 `Config` 路由，文件是 `configs/rginx.ron` 和 `configs/conf.d/default.ron`。
- 从示例配置里删除相关说明，文件是 `example/rginx.ron` 和 `example/conf.d/default.ron`。
- 在 README 中删除“HTTP 管理面”对外承诺，把运维方式改写成“本机命令 + 日志 + reload”。
- 在 ROADMAP 中把 `Status`、`Metrics`、`Config` 从能力矩阵挪到“已移除”或“非目标”。
- 这一阶段结束标准：默认安装后不会暴露任何内建管理路由。

## 阶段 2

删除配置模型里的管理面语义，切断新配置继续使用这些能力的入口。

- 从 `crates/rginx-config/src/model.rs` 删除 `HandlerConfig::Status`、`HandlerConfig::Metrics`、`HandlerConfig::Config`。
- 从 `crates/rginx-core/src/config/route.rs` 删除 `RouteAction::Status`、`RouteAction::Metrics`、`RouteAction::Config`。
- 从 `crates/rginx-config/src/compile/route.rs` 删除对应编译分支。
- 从 `crates/rginx-config/src/validate/route.rs` 删除 `Config` handler 的专用校验逻辑。
- 从 `crates/rginx-config/src/validate.rs` 和相关测试里删去“管理路由约束”的表述。
- 这一阶段结束标准：配置文件语法层面已经不能再声明这三类 handler。

## 阶段 3

删除 HTTP 处理链中的管理面实现，彻底收口运行时主路径。

- 删除或清空 `crates/rginx-http/src/handler/admin.rs` 中的 `status_response`、`metrics_response`、`config_response` 及相关辅助逻辑。
- 修改 `crates/rginx-http/src/handler/dispatch.rs`，移除 `RouteAction::Status`、`RouteAction::Metrics`、`RouteAction::Config` 分支。
- 清理 `crates/rginx-http/src/handler/mod.rs` 中与管理面绑定的常量、类型和测试引用。
- 修改 `crates/rginx-http/src/state.rs`，删除 `config_source`、`persistent_config_path`、`apply_config_source`、`replace_with_source` 这类只服务动态配置 API 的状态与方法。
- 修改 `crates/rginx-runtime/src/reload.rs`，热重载只负责从配置文件重读并替换，不再关心“活动配置源码回填”。
- 这一阶段结束标准：HTTP 服务只剩业务代理和 `Return` 之类纯请求路径能力，不再有内建管理面。

## 阶段 4

处理观测与运维的替代方案，避免“删完之后本地也什么都看不到”。

推荐分成必做和可选。

- 必做：保留并强化日志，把 access log 和 reload 日志作为默认运维入口，文件主要是 `crates/rginx-http/src/handler/access_log.rs` 和 `crates/rginx-observability/src/logging.rs`。
- 必做：保留 `rginx check` 和信号控制链路，入口在 `crates/rginx-app/src/main.rs` 和 `crates/rginx-app/src/cli.rs`。
- 可选：新增本地命令 `rginx status`，直接输出当前静态配置摘要和基础运行信息，不走 HTTP。
- 可选：新增本地命令 `rginx metrics` 或导出 `/run/rginx/*.prom`，如果仍然想保留 Prometheus 集成。
- 如果不做本地替代命令，就进一步删除 `crates/rginx-http/src/metrics.rs` 及其在代理链中的计数调用，保持系统更纯。
- 这一阶段要先做决策：是“只保留日志”，还是“日志 + 本地命令”。

## 阶段 5

大规模删测试，确保测试集反映新的产品边界，而不是拖着旧能力。

应删除整组管理面测试：

- `crates/rginx-app/tests/dynamic_config_api.rs`
- `crates/rginx-app/tests/active_health.rs` 中依赖 `/status` 或 `/metrics` 的断言要改成本地日志或内部测试方式
- `crates/rginx-app/tests/grpc_proxy.rs` 中通过 `/metrics` 观察状态的用例要改写
- `crates/rginx-app/tests/check.rs` 中与默认配置 `/status` `/metrics` 相关的预期要更新

应改写的单元测试：

- `crates/rginx-http/src/handler/mod.rs` 里的 `status_response`、`metrics_response`、`config` 相关测试
- `crates/rginx-http/src/metrics.rs` 的测试，如果最终不保留该模块则整文件可删
- `crates/rginx-http/src/state.rs` 里动态配置相关测试
- `crates/rginx-config/src/validate/tests.rs` 里管理 handler 约束测试
- `crates/rginx-config/src/compile.rs` 里 `Status`、`Metrics`、`Config` 编译测试

应保留并补强的测试：

- reload
- check
- vhost
- hardening
- failover
- gRPC/grpc-web
- ACL
- rate limit
- upstream health 本身，但不要再依赖公开状态页

## 阶段 6

最后做内部简化，真正把“控制面遗留”从架构里剔掉。

- 如果不再提供 HTTP 指标，删除 `crates/rginx-http/src/metrics.rs` 和相关调用，把计数逻辑缩回日志。
- 如果不再提供本地状态输出，进一步删除 `crates/rginx-http/src/state.rs` 中仅服务观测的冗余字段。
- 清理 README、ROADMAP、示例、测试名、错误信息里的 `admin/config api/status/metrics` 残留。
- 跑一遍 `cargo test --workspace`，然后再补一轮“默认配置启动 + reload + check”的手工回归。
- 这一阶段结束标准：仓库里不再存在“半删不删的控制面语义”。

## 建议执行顺序

- 第一轮只做阶段 1 到阶段 3，先把 HTTP 管理面彻底拿掉。
- 第二轮决定阶段 4 是“只留日志”还是“补本地命令”。
- 第三轮做阶段 5 到阶段 6，把测试和内部遗留清干净。

## 风险排序

- 最大风险不是代码删坏，而是“删了 HTTP 管理面后，团队没有替代运维方式”。
- 第二大风险是测试大量失效，但这是好事，因为它能强制清理旧产品边界。
- 最小风险是路由主路径；当前业务代理链和管理面分层已经比较清楚，删起来是可控的。
