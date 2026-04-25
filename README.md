# rginx

`rginx` 是一个面向 Linux 的 Rust 反向代理单二进制项目。

当前版本：`0.1.3-rc.13`

仓库现在只保留 `rginx` 主二进制及其直接依赖，不再包含控制面、浏览器 Console、节点 agent 或相关发布链路。

## 能力概览

- HTTP/1.1、HTTPS、HTTP/2、HTTP/3 入口
- Host / Path 路由
- `Return` 与反向代理处理器
- upstream `round_robin`、`ip_hash`、`least_conn`
- `weight`、`backup`、幂等请求 failover
- 下游 TLS、SNI、OCSP、mTLS、ALPN、TLS 版本控制
- 上游 TLS、mTLS、HTTP/2、HTTP/3、`server_name_override`
- gRPC、grpc-web、trailers、`grpc-timeout`
- 压缩、限流、CIDR allow/deny、`trusted_proxies`
- 热重载、优雅重启、平滑退出
- 本地只读运维命令：`status`、`snapshot`、`delta`、`wait`、`traffic`、`peers`、`upstreams`

## Workspace 结构

| 路径 | 作用 |
| --- | --- |
| `crates/rginx-app` | `rginx` 二进制、CLI、集成测试 |
| `crates/rginx-config` | 配置加载、校验、编译 |
| `crates/rginx-core` | 共享核心模型 |
| `crates/rginx-http` | HTTP server、proxy、TLS、HTTP/3、限流、状态快照 |
| `crates/rginx-runtime` | reload / restart / shutdown / health orchestration |
| `crates/rginx-observability` | tracing / logging 初始化 |
| `configs/` | 默认边缘代理配置 |
| `docs/` | HTTP/3、release gate 与阶段化文档 |
| `deploy/` | systemd / supervisor 示例 |
| `fuzz/` | `cargo-fuzz` targets、seed corpus、dictionary 与 coverage 入口 |
| `scripts/` | 安装、测试、发布、验证脚本 |

## 环境要求

- Rust `1.94.1`
- Linux：`rginx` 仅支持 Linux

## 快速开始

默认配置文件是 [configs/rginx.ron](/root/github/rginx/configs/rginx.ron)。

```bash
cargo run -p rginx -- -t
cargo run -p rginx -- check
cargo run -p rginx --
```

默认会加载：

- [configs/rginx.ron](/root/github/rginx/configs/rginx.ron)
- [configs/conf.d/default.ron](/root/github/rginx/configs/conf.d/default.ron)

常用命令：

```bash
rginx -t
rginx -s reload
rginx status
rginx snapshot --include status --include traffic
rginx traffic --window-secs 60
```

systemd / supervisor 示例：

- [rginx.service](/root/github/rginx/deploy/systemd/rginx.service)
- [rginx.conf](/root/github/rginx/deploy/supervisor/rginx.conf)

## 开发与验证

完整检查命令：

```bash
cargo check --workspace --all-targets --message-format short
cargo test --workspace --all-targets --no-fail-fast
cargo clippy --workspace --all-targets -- -D warnings
```

仓库内置脚本：

```bash
scripts/test-fast.sh
scripts/test-slow.sh
scripts/run-clippy-gate.sh
scripts/run-tls-gate.sh
scripts/run-http3-gate.sh
scripts/run-http3-release-gate.sh --soak-iterations 1
scripts/run-fuzz-smoke.sh --seconds 10
```

当前 fuzz harness 覆盖 5 个高风险输入面：

- `proxy_protocol`
- `config_preprocess`
- `ocsp_response`
- `certificate_inspect`
- `ocsp_responder_discovery`

版本化 `*.seed` 是 smoke / coverage 的默认输入，target-specific 字典和 libFuzzer 参数位于 `fuzz/dictionaries/` 与 `fuzz/options/`。详细说明见 `fuzz/README.md`。

## 构建与交付

- [scripts/install.sh](/root/github/rginx/scripts/install.sh) / [scripts/uninstall.sh](/root/github/rginx/scripts/uninstall.sh) 用于安装与卸载
- release workflow 只发布 `rginx` Linux 归档和校验文件
- 预发布 tag 会在 release verify 和 [prepare-release.sh](/root/github/rginx/scripts/prepare-release.sh) 里额外执行 `./scripts/run-fuzz-smoke.sh --seconds 10`

如果仓库根目录存在 `RELEASE_NOTES_<tag>.md`，release workflow 会优先把它拼进 GitHub Release 正文。

## 许可证

双许可证：

- [LICENSE-MIT](/root/github/rginx/LICENSE-MIT)
- [LICENSE-APACHE](/root/github/rginx/LICENSE-APACHE)
