# rginx

`rginx` 是一个面向 Linux 的 Rust 边缘反向代理单二进制项目。

当前版本：`0.1.3`

## 能力概览

- HTTP/1.1、HTTPS、HTTP/2、HTTP/3 入口
- Host / Path 路由与 `Return` / 反向代理处理器
- upstream `round_robin`、`ip_hash`、`least_conn`
- `weight`、`backup`、幂等请求 failover
- 下游 TLS、SNI、OCSP、mTLS、ALPN、TLS 版本控制
- 上游 TLS、mTLS、HTTP/2、HTTP/3、`server_name_override`
- gRPC、grpc-web、trailers、`grpc-timeout`
- 压缩、限流、CIDR allow/deny、`trusted_proxies`
- 热重载、优雅重启、平滑退出
- 本地只读运维命令：`check`、`status`、`snapshot`、`snapshot-version`、`delta`、`wait`、`counters`、`traffic`、`peers`、`upstreams`

## 平台与交付

- 仅支持 Linux
- 源码构建要求 Rust `1.94.1`
- GitHub Release 提供 `linux-amd64` 和 `linux-arm64` 归档
- release archive 包含 `rginx`、`configs/`、`scripts/` 以及 `deploy/systemd` / `deploy/supervisor` 示例

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
| `docs/` | 当前生效的仓库治理文档 |
| `deploy/` | systemd / supervisor 示例 |
| `fuzz/` | `cargo-fuzz` targets、seed corpus、dictionary 与 coverage 入口 |
| `scripts/` | 安装、测试与验证脚本 |

## 快速开始

默认配置文件是 [configs/rginx.ron](configs/rginx.ron)。

```bash
cargo run -p rginx -- -t
cargo run -p rginx -- check
cargo run -p rginx --
```

默认会加载：

- [configs/rginx.ron](configs/rginx.ron)
- [configs/conf.d/default.ron](configs/conf.d/default.ron)

常用命令：

```bash
rginx -t
rginx -s reload
rginx check
rginx status
rginx snapshot --include status --include traffic
rginx snapshot-version
rginx delta --since-version <version> --include status
rginx wait --since-version <version> --timeout-ms 5000
rginx counters
rginx peers
rginx traffic --window-secs 60
rginx upstreams --window-secs 60
```

systemd / supervisor 示例：

- [rginx.service](deploy/systemd/rginx.service)
- [rginx.conf](deploy/supervisor/rginx.conf)

## 安装

从源码仓库安装：

```bash
./scripts/install.sh --mode source
```

直接安装最新稳定版 release：

```bash
curl -fsSL https://raw.githubusercontent.com/vansour/rginx/main/scripts/install.sh | \
  bash -s -- --mode release --version latest
```

卸载入口：

```bash
./scripts/uninstall.sh
```

## 开发与验证

完整检查命令：

```bash
cargo check --workspace --all-targets --message-format short
cargo test --workspace --all-targets --no-fail-fast
cargo clippy --workspace --all-targets -- -D warnings
```

仓库内置脚本：

```bash
python3 scripts/run-modularization-gate.py
scripts/test-fast.sh
scripts/test-slow.sh
scripts/run-clippy-gate.sh
scripts/run-tls-gate.sh
scripts/run-http3-gate.sh
scripts/run-http3-release-gate.sh --soak-iterations 1
scripts/run-fuzz-smoke.sh --seconds 10
scripts/run-fuzz-coverage.sh --target certificate_inspect
```

当前 fuzz harness 覆盖 5 个高风险输入面：

- `proxy_protocol`
- `config_preprocess`
- `ocsp_response`
- `certificate_inspect`
- `ocsp_responder_discovery`

版本化 `*.seed` 是 smoke / coverage 的默认输入，target-specific 字典和 libFuzzer 参数位于 `fuzz/dictionaries/` 与 `fuzz/options/`。详细说明见 `fuzz/README.md`。

## 文档

- [docs/README.md](docs/README.md) 汇总当前生效的仓库治理文档
- [fuzz/README.md](fuzz/README.md) 说明 fuzz target、seed、smoke 和 coverage 流程

## 许可证

双许可证：

- [LICENSE-MIT](LICENSE-MIT)
- [LICENSE-APACHE](LICENSE-APACHE)
