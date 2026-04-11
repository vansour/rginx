# rginx

`rginx` 是一个面向中小规模部署的 Rust 入口反向代理。

当前版本：`v0.1.3-rc.8`

它的目标很收口：

- 做 HTTP / HTTPS 入口反代
- 做 API gateway 前置代理
- 做 gRPC ingress 和 grpc-web 入口转换
- 做 LB / CDN 后方的边缘反代
- 稳定覆盖 TLS 终止、Host/Path 路由、上游代理、健康检查、热重载、优雅重启、基础流量治理和本地可观测性

它不是当前阶段的目标：

- 不是静态文件服务器
- 不是公网远程管理面
- 不是完整 nginx DSL 兼容层
- 不是 `stream` / `mail` / FastCGI / uwsgi 入口代理
- 不是“所有场景都能替代 nginx”的 drop-in replacement

## 当前能力

- RON 配置加载、相对 `include` 和字符串环境变量展开
- 单进程多 worker 运行时，支持 `worker_threads` 和 `accept_workers`
- 兼容旧 `server.listen` 单入口模型
- 支持显式多监听模型 `listeners: []`
- `Exact("/foo")` / `Prefix("/api")` 路由匹配
- 按 `grpc_service` / `grpc_method` 细分 gRPC 路由
- `Proxy` / `Return` 两种处理器
- upstream `round_robin` / `ip_hash` / `least_conn`
- peer `weight` / `backup`
- 幂等或可重放请求的 failover
- 入站 HTTP/2（TLS / ALPN）
- 下游 TLS 版本控制（`TLS1.2` / `TLS1.3`）
- 下游 ALPN 控制
- 无 SNI 客户端的默认证书回退
- 动态 OCSP 拉取、缓存与刷新（基于证书 AIA + `ocsp_staple_path` 缓存文件）
- 下游 mTLS 客户端证书校验（`Optional` / `Required`）
- 上游 mTLS 客户端证书
- 上游 TLS 版本控制
- 上游证书校验深度和静态 CRL
- 上游 HTTP/2（HTTPS / TLS / ALPN）
- 基础 gRPC over HTTP/2 代理
- 基础 grpc-web binary / text 转换
- 请求 / 响应 trailers 透传
- `grpc-timeout` deadline
- 本地代理错误到 `grpc-status` / `grpc-message` 的转换
- 非法 `grpc-web-text` 请求体到 `grpc-status = 3` 的拒绝
- 下游提前取消时 `grpc-status = 1` 可观测性
- br / gzip 响应压缩协商
- HTTP/1.1 `Upgrade` / WebSocket 透传
- listener / server 级超时治理
- upstream 细粒度超时、连接池、TCP / HTTP/2 keepalive 调优
- 被动健康检查
- 主动 HTTP 健康检查
- 主动 gRPC health check
- 主动健康检查按 peer 的稳定初始 jitter
- 路由级 CIDR allow / deny
- 路由级限流
- `trusted_proxies`
- 入站 `PROXY protocol` v1
- hostname upstream peer，新建连接时可重新解析
- 自动透传或生成 `X-Request-ID`
- 自定义 access log 模板
- `Ctrl-C` / `SIGTERM` / `SIGQUIT` 平滑退出
- `SIGHUP` 热重载
- Linux 下显式 fd 继承式优雅重启
- `rginx check`
- `rginx migrate-nginx`
- 本地只读运维面：`snapshot / snapshot-version / delta / wait / status / counters / traffic / peers / upstreams`

## 仓库结构

| 路径 | 作用 |
| --- | --- |
| `crates/rginx-app` | 二进制入口、CLI、集成测试 |
| `crates/rginx-config` | 配置加载、校验、编译 |
| `crates/rginx-core` | 共享运行时模型与基础类型 |
| `crates/rginx-http` | HTTP server、handler、proxy、TLS、限流、指标 |
| `crates/rginx-runtime` | 运行时编排、reload、restart、admin、active health |
| `crates/rginx-observability` | tracing / logging 初始化 |
| `configs/` | 默认活跃配置目录镜像 |
| `example/` | 更完整的配置参考 |
| `docs/` | 维护与重构类开发文档，例如 `refactor-plan.md` |
| `deploy/` | systemd / supervisor 示例 |
| `scripts/` | 安装、卸载、`.deb` 打包、APT 仓库发布、benchmark、soak、release 脚本 |

主路径大致是：

`CLI -> load_and_compile -> ConfigSnapshot -> SharedState -> accept loop -> handler::dispatch -> route action -> access log`

## 快速开始

### 环境

- Linux
- Rust `1.85+`

### 源码运行

仓库默认配置在 `configs/rginx.ron`：

```bash
cargo run -p rginx -- --config configs/rginx.ron
```

启动前先检查配置：

```bash
cargo run -p rginx -- check --config configs/rginx.ron
cargo run -p rginx -- -t --config configs/rginx.ron
```

如果先构建：

```bash
cargo build -p rginx
./target/debug/rginx --config configs/rginx.ron
./target/debug/rginx check --config configs/rginx.ron
./target/debug/rginx status --config configs/rginx.ron
```

### 测试分层

阶段 0 之后，仓库把测试入口分成了快测和慢测两层：

```bash
./scripts/test-fast.sh
./scripts/test-slow.sh
```

- `test-fast.sh` 运行 `rginx-core`、`rginx-config`、`rginx-http`、`rginx-runtime`、`rginx-observability` 的 crate 内测试，以及 `rginx` 二进制本身的单测。
- `test-slow.sh` 运行 `crates/rginx-app/tests/` 下的集成测试。
- `scripts/run-tls-gate.sh` 继续保留给 TLS 相关回归门禁和发布前检查。

### 热更新边界

当前 `SIGHUP` 热重载支持：

- 路由、vhost、upstream、TLS 相关业务配置变更
- 显式 `listeners: []` 模型下的 listener 新增与删除
- `include` 片段更新

当前仍然要求显式重启的字段：

- `listen`
- `listeners[].listen`
- `runtime.worker_threads`
- `runtime.accept_workers`

也就是说，显式 listener 可以热增删，但既有 listener 的 `listen` 地址变化仍属于 restart boundary。

更完整的运行时语义见：

- `docs/reload-semantics.md`
- `docs/runtime-architecture.md`

### 安装

从当前源码仓库安装：

```bash
./scripts/install.sh --mode source
```

从 GitHub Release 安装：

```bash
curl -fsSL https://raw.githubusercontent.com/vansour/rginx/main/scripts/install.sh | \
  bash -s -- --mode release --version <tag>
```

常用参数：

```bash
./scripts/install.sh --mode source --force
```

安装后默认路径：

- 二进制：`/usr/sbin/rginx`
- 主配置：`/etc/rginx/rginx.ron`
- 站点片段：`/etc/rginx/conf.d/*.ron`
- pid：`/run/rginx.pid`
- admin socket：`/run/rginx/admin.sock`

卸载：

```bash
rginx-uninstall
rginx-uninstall --purge-config
```

### APT / Debian 安装

如果你想要和 `nginx` 一样走 `apt install` / `apt remove` / `apt purge` 这条链路，仓库里现在已经补了 `.deb` 打包和 APT 仓库发布脚本。

先在源码仓库里构建 Debian 包：

```bash
./scripts/build-deb.sh
```

默认会产出：

```bash
target/debian/rginx_<version>_<arch>.deb
```

本机通过 `apt` 安装本地包：

```bash
sudo apt install ./target/debian/rginx_<version>_<arch>.deb
```

安装后：

- 二进制：`/usr/sbin/rginx`
- systemd unit：`/lib/systemd/system/rginx.service`
- 主配置：`/etc/rginx/rginx.ron`
- 站点片段：`/etc/rginx/conf.d/*.ron`

APT 生命周期：

```bash
sudo apt remove rginx
sudo apt purge rginx
```

语义和 `nginx` 一样：

- `apt remove rginx` 会卸载程序，但保留 `/etc/rginx/*` 配置
- `apt purge rginx` 会连同 `/etc/rginx/*` 一起清掉

注意：

- APT 安装场景下不要再用 `rginx-uninstall`，应该让 `apt remove/purge` 接管整个生命周期
- 包的 `postinst` 会创建 `rginx` system user，并执行 `systemctl daemon-reload`
- 是否 `enable --now rginx` 仍建议由你显式控制

首次启用服务：

```bash
sudo systemctl enable --now rginx
sudo systemctl status rginx
```

### 发布成 APT 仓库

如果你希望最终用户直接：

```bash
sudo apt update
sudo apt install rginx
```

那还需要把 `.deb` 发布成一个签名过的 APT 仓库。仓库里提供了：

```bash
./scripts/publish-apt-repo.sh
```

示例：

```bash
./scripts/build-deb.sh
./scripts/publish-apt-repo.sh \
  --repo-root ./target/apt-repo \
  --deb-dir ./target/debian \
  --gpg-key packages@example.com \
  --export-key ./target/apt-repo/rginx-archive-keyring.gpg
```

这会生成标准静态仓库结构：

```text
target/apt-repo/
  pool/
  dists/stable/
    Release
    InRelease
    Release.gpg
    main/binary-amd64/Packages{,.gz}
    ...
```

把这个目录挂到 HTTP/HTTPS 静态站点后，客户端就可以这样接入：

```bash
curl -fsSL https://packages.example.com/rginx/rginx-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/rginx-archive-keyring.gpg >/dev/null

echo "deb [signed-by=/usr/share/keyrings/rginx-archive-keyring.gpg] https://packages.example.com/rginx stable main" \
  | sudo tee /etc/apt/sources.list.d/rginx.list

sudo apt update
sudo apt install rginx
```

### GitHub Actions 自动发布

仓库里的 [release.yml](/root/github/rginx/.github/workflows/release.yml) 现在已经扩成完整 release 流水线：

- tag `v*` 时先校验 tag 和跑测试
- 在 `ubuntu-24.04` 上产出 `linux-amd64` tarball 和 `amd64 .deb`
- 在 `ubuntu-24.04-arm` 上产出 `linux-arm64` tarball 和 `arm64 .deb`
- 把 tarball、`.deb` 和 `SHA256SUMS.txt` 上传到 GitHub Release
- 对稳定 tag 额外生成签名过的 APT 仓库并发布到 GitHub Pages

当前约定：

- 预发布 tag，例如 `v0.1.3-rc.8`：发布 GitHub Release 资产，但不更新 APT 仓库
- 稳定 tag，例如 `v0.1.3`：同时发布 GitHub Release 和 GitHub Pages APT 仓库

要让稳定版自动发布 APT 仓库，还需要一次性配置：

1. 在仓库 `Settings -> Pages` 里把 Source 设成 `GitHub Actions`
2. 在仓库 `Settings -> Secrets and variables -> Actions` 里配置这些 secrets

必需 secrets：

- `APT_GPG_PRIVATE_KEY`
  内容是 ASCII-armored 的私钥块，用来签 `Release` / `InRelease`
- `APT_GPG_KEY_ID`
  用于签名的 key id、fingerprint 或 email

可选 secret：

- `APT_GPG_PASSPHRASE`
  如果私钥带 passphrase，就配置这个；无口令私钥可以不配

稳定版自动发布成功后，默认 GitHub Pages URL 通常是：

```text
https://<owner>.github.io/<repo>/apt
```

然后用户就可以按前面的方式接入：

```bash
curl -fsSL https://<owner>.github.io/<repo>/apt/rginx-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/rginx-archive-keyring.gpg >/dev/null

echo "deb [signed-by=/usr/share/keyrings/rginx-archive-keyring.gpg] https://<owner>.github.io/<repo>/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/rginx.list

sudo apt update
sudo apt install rginx
```

## 配置概览

配置格式是 RON。

### 兼容旧模型

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
        worker_threads: None,
        accept_workers: None,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:80",
        server_names: [],
        trusted_proxies: [],
        tls: None,
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("ok\n"),
            ),
        ),
    ],
    servers: [],
)
```

### 显式多监听模型

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    listeners: [
        ListenerConfig(
            name: "http",
            listen: "0.0.0.0:80",
        ),
        ListenerConfig(
            name: "https",
            listen: "0.0.0.0:443",
            tls: Some(ServerTlsConfig(
                cert_path: "/etc/rginx/certs/default.crt",
                key_path: "/etc/rginx/certs/default.key",
                versions: Some([Tls12, Tls13]),
                cipher_suites: Some([Tls13Aes256GcmSha384, Tls13Aes128GcmSha256]),
                key_exchange_groups: Some([X25519, Secp256r1]),
                session_resumption: Some(true),
                session_tickets: Some(false),
                session_cache_size: Some(256),
                session_ticket_count: Some(2),
            )),
        ),
    ],
    server: ServerConfig(
        server_names: ["example.com"],
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("ok\n"),
            ),
        ),
    ],
    servers: [],
)
```

### 默认仓库配置

- `configs/rginx.ron`
- `configs/conf.d/default.ron`

更完整参考：

- `example/rginx.ron`
- `example/conf.d/default.ron`

### 预处理能力

支持两类轻量预处理：

- 独立一行的 `// @include "relative/path.ron"`
- 独立一行的 `// @include "conf.d/*.ron"`
- 双引号字符串里的 `${VAR}` 和 `${VAR:-default}`

示例：

```ron
server: ServerConfig(
    listen: "${rginx_listen:-0.0.0.0:80}",
),
servers: [
    // @include "conf.d/*.ron"
],
```

边界：

- `include` 路径相对当前配置文件所在目录
- 通配目前只支持 `*.ron`
- 环境变量只在普通双引号字符串里展开
- 缺失的 `${VAR}` 直接报错
- 需要字面量 `$` 时写 `$$`

## 常用运维命令

### 配置检查

```bash
rginx check --config /etc/rginx/rginx.ron
rginx -t --config /etc/rginx/rginx.ron
```

成功输出会带上：

- `listener_model=legacy|explicit`
- `listeners=<count>`
- `reload_requires_restart_for=listen,listeners[].listen,runtime.worker_threads,runtime.accept_workers`
- `reload_tls_updates=...`
- `tls_expiring_certificates=...`

这些字段的作用很直接：

- 告诉你当前是兼容旧 listener 还是显式 listener 模型
- 告诉你当前 listener 数量
- 明确哪些字段属于启动期边界，后续不能靠 `reload` 热替换
- 明确哪些 TLS 字段可以通过 `reload` 热更新
- 提前暴露即将过期的证书

### reload / restart / stop

```bash
rginx -s reload
rginx -s restart
rginx -s quit
rginx -s stop
```

也可以直接发信号：

```bash
kill -HUP <pid>
kill -TERM <pid>
kill -QUIT <pid>
```

建议这样理解：

- `reload`
  - 适用于路由、upstream、ACL、限流、vhost 级 TLS 这类可原地替换项
- `restart`
  - 适用于监听地址、listener 集合、`runtime.worker_threads`、`runtime.accept_workers` 这类启动期结构变化

如果你对启动期边界字段发送 `reload`：

- 失败原因会进入运行日志
- `rginx status` 的 `last_reload` 会带上具体变化字段

### 本地只读状态

常用命令：

```bash
rginx snapshot
rginx snapshot --include traffic --include upstreams
rginx snapshot-version
rginx delta --since-version 12
rginx wait --since-version 12 --timeout-ms 5000
rginx status
rginx counters
rginx traffic --window-secs 300
rginx peers
rginx upstreams --window-secs 300
```

当前有两种稳定输出口径：

- 文本型命令：
  - `status / counters / traffic / peers / upstreams`
  - 每行一条记录
  - `kind=<record-type> key=value ...`
- 结构化命令：
  - `snapshot / delta`
  - pretty JSON

文本型输出示例：

```text
kind=status revision=3 listen=127.0.0.1:8080 active_connections=0 reload_failures=1
kind=counters downstream_requests_total=42 downstream_responses_2xx_total=40
kind=traffic_listener listener=default downstream_requests_total=42 grpc_requests_total=3
kind=peer_health_peer upstream=backend peer=http://127.0.0.1:9000 available=true
kind=upstream_stats upstream=backend peer_attempts_total=42 failovers_total=1
```

现在 `status` / `snapshot` 里也会包含 TLS 运行时视图，例如：

- listener 是否启用 TLS
- SNI 名称集合
- 证书路径与到期时间
- mTLS listener 分布和握手失败分类计数

## benchmark / soak

固定 benchmark 矩阵：

```bash
python3 scripts/run-benchmark-matrix.py \
  --http1-url http://127.0.0.1:18080/ \
  --https-url https://127.0.0.1:18443/ \
  --http2-url https://127.0.0.1:18443/ \
  --grpc-url https://127.0.0.1:18443/grpc.health.v1.Health/Check \
  --grpc-web-url http://127.0.0.1:18080/grpc.health.v1.Health/Check \
  --grpc-web-text-url http://127.0.0.1:18080/grpc.health.v1.Health/Check \
  --requests 200 \
  --concurrency 16
```

固定 soak 矩阵：

```bash
./scripts/run-soak.sh --iterations 1
```

当前建议至少用下面三条命令把工作区收口成可继续迭代的稳定基线：

```bash
./scripts/test-fast.sh
./scripts/test-slow.sh
./scripts/run-soak.sh --iterations 1
```

### TLS Release Gate

每次改动 TLS 相关逻辑，发布前至少确认：

```bash
./scripts/test-fast.sh
./scripts/test-slow.sh
./scripts/run-tls-gate.sh
./scripts/run-soak.sh --iterations 1
rginx check --config /etc/rginx/rginx.ron
```

建议把这几项当成 TLS 子系统的最小发布门槛：

- 下游 TLS / SNI / 默认证书回退通过
- 下游 mTLS 通过
- 上游 HTTPS / mTLS / HTTP2 / SNI 通过
- access log / admin / check 的 TLS 可观测性通过
- reload / restart 边界没有回归
- `migrate-nginx` 对常见 SSL 指令仍能给出稳定结果或 warning

## 部署

仓库内提供：

- `deploy/systemd/rginx.service`
- `deploy/supervisor/rginx.conf`

systemd：

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now rginx
sudo systemctl reload rginx
sudo systemctl restart rginx
sudo systemctl status rginx
```

supervisor：

```bash
sudo supervisorctl reread
sudo supervisorctl update
sudo supervisorctl restart rginx
sudo supervisorctl status rginx
```

上线前至少确认：

```bash
rginx check --config /etc/rginx/rginx.ron
rginx snapshot --config /etc/rginx/rginx.ron
rginx status --config /etc/rginx/rginx.ron
rginx counters --config /etc/rginx/rginx.ron
rginx traffic --config /etc/rginx/rginx.ron
rginx peers --config /etc/rginx/rginx.ron
rginx upstreams --config /etc/rginx/rginx.ron
```

## 当前限制

- Linux only
- 入站 HTTP/2 只支持 TLS / ALPN，不支持明文 `h2c`
- 上游 HTTP/2 当前要求 `https://` peer，不支持明文 `h2c`
- active gRPC health check 当前也要求 `https://` peer
- 上游 TLS 名称策略目前分两层：`server_name` 控制是否发送 SNI，`server_name_override` 控制证书校验目标 / SNI 覆盖名
- reload 不能修改：
  - `listen`
  - listener 集合
  - `runtime.worker_threads`
  - `runtime.accept_workers`
- body limit 当前是 listener / server 级，不是 route 级
- `PROXY protocol` 当前只支持 inbound v1
- upstream peer 只接受 `scheme://authority`，不接受 path / query
- 当前只承诺基础 `grpc-web` binary / text 模式，不承诺更完整高级兼容语义

## 参考入口

- 默认活跃配置镜像：`configs/`
- 更完整配置参考：`example/`
- 部署示例：`deploy/`
- 安装、release、benchmark、soak：`scripts/`

## 许可证

`MIT OR Apache-2.0`
