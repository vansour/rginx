# rginx

`rginx` 是一个 Rust monorepo，当前包含三条主线：

- `rginx`：Linux 上的边缘反向代理二进制
- `rginx-web`：控制面 Web 进程，内含 API、浏览器 Console、后台 worker，以及可选权威 DNS 运行时
- `rginx-node-agent`：节点本地 agent，负责和控制面同步发布、状态与快照

当前版本：`0.1.3-rc.11`

本仓库不再维护额外文档目录；根 `README.md` 是唯一持续更新的说明入口。

## 能力概览

### 边缘代理

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

### 控制面

- `rginx-web` 提供统一 Web 入口
- 浏览器 Console 由 `rginx-control-console` 编译为 WASM 后静态嵌入
- Postgres 负责持久化，Dragonfly 负责热路径状态与任务协同
- 后台 worker 负责部署推进、状态同步与轮询编排
- 可选权威 DNS 运行时可直接由 `rginx-web` 挂载

### 节点联动

- `rginx-node-agent` 负责节点注册、心跳、任务拉取与结果回传
- agent 可对接本地 `admin.sock`，回传 runtime snapshot
- 控制面与 agent 共享 DTO 位于 `rginx-control-types`

## Workspace 结构

| 路径 | 作用 |
| --- | --- |
| `crates/rginx-app` | `rginx` 二进制、CLI、集成测试 |
| `crates/rginx-config` | 配置加载、校验、编译 |
| `crates/rginx-core` | 共享核心模型 |
| `crates/rginx-http` | HTTP server、proxy、TLS、HTTP/3、限流、状态快照 |
| `crates/rginx-runtime` | reload / restart / shutdown / health orchestration |
| `crates/rginx-observability` | tracing / logging 初始化 |
| `crates/rginx-web` | 控制面 Web 进程 |
| `crates/rginx-control-console` | Dioxus 控制台前端 |
| `crates/rginx-control-service` | 控制面业务服务层 |
| `crates/rginx-control-store` | 控制面数据访问层 |
| `crates/rginx-control-types` | 控制面与 agent 共享类型 |
| `crates/rginx-node-agent` | 节点 agent |
| `crates/rginx-dns` | 权威 DNS 运行时基础能力 |
| `configs/` | 默认边缘代理配置 |
| `deploy/` | systemd / supervisor 示例 |
| `docker/` | Docker 相关构建目录 |
| `scripts/` | 安装、测试、发布、验证脚本 |

## 环境要求

- Rust `1.94.1`
- Linux：`rginx` 边缘代理仅支持 Linux
- Docker / Docker Compose：本地控制面联调推荐使用
- 控制面依赖 Postgres 与 Dragonfly

## 快速开始

### 本地验证边缘代理

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

### 本地启动控制面

先准备环境变量：

```bash
cp .env.example .env
```

再启动：

```bash
docker compose up --build -d
```

默认入口：

- Console / API: `http://127.0.0.1:8080`
- Postgres: `127.0.0.1:5432`
- Dragonfly: `127.0.0.1:6379`

本地开发默认管理员账号来自 [.env.example](/root/github/rginx/.env.example)：

- username: `admin`
- password: `admin`

如果要同时启用权威 DNS，在 `.env` 中设置：

- `RGINX_CONTROL_DNS_UDP_ADDR`
- `RGINX_CONTROL_DNS_TCP_ADDR`

### 节点 agent

节点侧 systemd 示例位于：

- [rginx.service](/root/github/rginx/deploy/systemd/rginx.service)
- [rginx-node-agent.service](/root/github/rginx/deploy/control-plane/systemd/rginx-node-agent.service)
- [rginx-node-agent.env.example](/root/github/rginx/deploy/control-plane/systemd/rginx-node-agent.env.example)

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
scripts/test-control-plane-compose.sh
scripts/test-control-console-e2e.sh
```

## 构建与交付

- [Dockerfile](/root/github/rginx/Dockerfile) 当前提供 `rginx-web` 镜像构建
- [compose.yaml](/root/github/rginx/compose.yaml) 是本地控制面一体化入口
- [scripts/install.sh](/root/github/rginx/scripts/install.sh) / [scripts/uninstall.sh](/root/github/rginx/scripts/uninstall.sh) 用于边缘节点安装与卸载

## 许可证

双许可证：

- [LICENSE-MIT](/root/github/rginx/LICENSE-MIT)
- [LICENSE-APACHE](/root/github/rginx/LICENSE-APACHE)
