# Quick Start

本页目标是让你在几分钟内把 `Rginx` 跑起来，并验证一条最小可用的反向代理链路。

## 前置条件

- 本机可以绑定一个监听端口
- 如果你要从源码构建，Rust 工具链需要可用
- 如果你要测试热重载，建议在 Unix 环境下运行，因为 `SIGHUP` 只在 Unix 上启用

## 一键安装与卸载

源码目录下默认配置文件是 `configs/rginx.ron`。安装版会优先尝试 `<prefix>/etc/rginx/rginx.ron`。如果你安装时用了自定义 `--config-dir`，运行时请继续通过 `RGINX_CONFIG` 或 `--config` 显式指定。

从源码仓库安装：

```bash
./scripts/install.sh --mode source
```

安装指定 GitHub Release 版本：

```bash
curl -fsSL https://raw.githubusercontent.com/vansour/rginx/main/scripts/install.sh | bash -s -- --mode release --version <tag>
```

其中 `latest` 只会解析最新稳定版；如果你要安装预发布版，请显式传入具体 tag，例如 `v0.1.1-rc.2`。

默认安装位置：

- `<prefix>/bin/rginx`
- `<prefix>/bin/rginx-uninstall`
- `<prefix>/etc/rginx/rginx.ron`
- `<prefix>/share/rginx/configs`

卸载：

```bash
rginx-uninstall
rginx-uninstall --purge-config
```

安装完成后，默认配置路径可以直接这样验证：

```bash
rginx check
rginx
```

## 构建与启动

直接运行默认配置：

```bash
cargo run -p rginx -- --config configs/rginx.ron
```

先检查配置再启动：

```bash
cargo run -p rginx -- check --config configs/rginx.ron
cargo run -p rginx -- --config configs/rginx.ron
```

如果你已经构建过：

```bash
cargo build -p rginx
./target/debug/rginx check --config configs/rginx.ron
./target/debug/rginx --config configs/rginx.ron
```

## 最小配置

下面是一份最小可运行配置：

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        listen: "127.0.0.1:8080",
    ),
    upstreams: [
        UpstreamConfig(
            name: "backend",
            peers: [
                UpstreamPeerConfig(
                    url: "http://127.0.0.1:9000",
                ),
            ],
        ),
    ],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Static(
                body: "Rginx is running.\n",
            ),
        ),
        LocationConfig(
            matcher: Prefix("/api"),
            handler: Proxy(
                upstream: "backend",
            ),
        ),
    ],
    servers: [],
)
```

## 配置检查命令做了什么

`rginx check` 不只是语法检查。它会：

- 读取并解析 RON 配置
- 执行语义校验
- 编译成运行时 `ConfigSnapshot`
- 初始化一轮运行时依赖，提前发现 TLS、upstream、路由等问题

成功时会输出类似：

```text
configuration is valid: listen=127.0.0.1:8080 tls=disabled vhosts=1 routes=2 upstreams=1
```

## 仓库自带示例配置

| 文件 | 场景 |
| --- | --- |
| `configs/rginx.ron` | 基础代理、状态页、指标页 |
| `configs/rginx-ip-hash-example.ron` | 基于客户端 IP 的粘性转发 |
| `configs/rginx-least-conn-example.ron` | 最少连接数转发 |
| `configs/rginx-weighted-example.ron` | 加权分流 |
| `configs/rginx-backup-example.ron` | 主备 upstream |
| `configs/rginx-https-example.ron` | 上游 HTTPS |
| `configs/rginx-https-custom-ca-example.ron` | 自定义 CA |
| `configs/rginx-https-insecure-example.ron` | 开发环境跳过证书校验 |
| `configs/rginx-vhosts-example.ron` | 多虚拟主机 |

## 第一次验证建议

启动后，至少做下面几件事：

1. 访问静态根路径，确认监听和路由正常。
2. 访问 `/status`，确认配置被正确加载。
3. 访问 `/metrics`，确认可观测性链路可用。
4. 如果配置了 upstream，再请求一条代理路径，确认回源正常。

例如：

```bash
curl -i http://127.0.0.1:8080/
curl -i http://127.0.0.1:8080/status
curl -i http://127.0.0.1:8080/metrics
```

## 热重载与退出

普通退出：

- `Ctrl-C`
- `SIGTERM`

热重载：

```bash
kill -HUP <rginx-pid>
```

注意：

- 热重载不会中断现有连接
- 监听地址变更不支持热重载，必须重启

## 下一步阅读

- [Configuration](Configuration.md)
- [Routing and Handlers](Routing-and-Handlers.md)
- [Upstreams](Upstreams.md)
