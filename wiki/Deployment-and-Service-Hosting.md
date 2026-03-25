# Deployment and Service Hosting

本页把“怎么安装、怎么隔离管理接口、怎么交给外部 supervisor 托管”收成一套当前可执行的建议。

先说边界：

- `rginx` 当前不内置 systemd、launchd 或其他服务单元
- 进程生命周期默认应交给外部 supervisor，例如 systemd、容器运行时或编排系统
- 当前单实例只围绕一个 `listen` 工作，不支持把业务流量和管理流量拆到两个监听地址

这意味着部署重点不是“找内建托管器”，而是：

1. 明确安装目录和活跃配置路径
2. 把 `/status`、`/metrics`、`Config` 管理路由限制在受控网络
3. 把启动、重载、退出交给外部托管系统

## 安装布局

默认安装脚本会写入：

- `<prefix>/bin/rginx`
- `<prefix>/bin/rginx-uninstall`
- `<prefix>/etc/rginx/rginx.ron`
- `<prefix>/share/rginx/configs`
- `<prefix>/share/doc/rginx`

如果你使用了自定义 `--config-dir`，请注意两件事：

- 运行时继续通过 `--config <path>` 或环境变量 `rginx_config=<path>` 指向那份活跃配置
- service unit 里不要再假设默认配置路径仍在 `<prefix>/etc/rginx/rginx.ron`

源码安装示例：

```bash
./scripts/install.sh --mode source --prefix /opt/rginx
```

Release 安装示例：

```bash
curl -fsSL https://raw.githubusercontent.com/vansour/rginx/main/scripts/install.sh | \
  bash -s -- --mode release --version v0.1.2-rc.1 --prefix /opt/rginx
```

## 管理接口隔离

当前没有多 `listen`，所以最稳妥的做法是：

- 业务入口和管理入口共用同一个监听地址
- 通过 `allow_cidrs` 明确限制 `/status`、`/metrics`、`/-/config`
- 再由主机防火墙、VPC ACL、Kubernetes NetworkPolicy、内网 LB 或 VPN 做第二层隔离

仓库里已经提供了一个更贴近生产习惯的示例：

- [`configs/rginx-admin-example.ron`](../configs/rginx-admin-example.ron)

它演示了三种不同隔离强度：

- `/status`：只允许 loopback 和内网 CIDR
- `/metrics`：只允许 loopback 和内网抓取器网段
- `/-/config`：比状态页更严格，只允许 loopback 和更小的运维网段

如果你的前面还有 LB、CDN 或入口网关，务必同时配好 `trusted_proxies`。否则：

- access log 里的客户端 IP 会失真
- ACL 会按错误的客户端地址判断
- 限流和 `ip_hash` 也会基于错误地址工作

## 当前可行的变更流程

动态配置 API 目前只支持完整文档替换，不支持 patch 或 staged update。当前更稳妥的流程是：

1. 先在候选配置上执行 `rginx check --config <candidate>`
2. 选择一种切换方式：
   - 通过 `PUT /-/config` 提交完整 RON 文档
   - 或者原子替换配置文件后发送 `SIGHUP`
3. 切换后立即检查：
   - `/status` 的 `revision`
   - `/metrics` 里的 `rginx_config_reloads_total`
   - access log / error log 是否出现新的拒绝、超时或健康检查异常

如果改动涉及下面这些字段，就不要尝试热切换：

- `listen`
- `runtime.worker_threads`
- `runtime.accept_workers`

这些变更必须走重启流程。

## systemd 示例

下面是一份“外部托管、显式配置路径、支持热重载”的最小示例。它只是建议模板，不是仓库内建单元：

```ini
[Unit]
Description=rginx reverse proxy
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/rginx/bin/rginx --config /opt/rginx/etc/rginx/rginx.ron
ExecReload=/bin/kill -HUP $MAINPID
KillSignal=SIGTERM
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info,rginx_http=info
NoNewPrivileges=true
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
```

建议配套操作：

- 启动前先单独执行一次 `rginx check --config ...`
- 用 `systemctl reload` 只做可热更新的配置变更
- 如果改了 `listen` 或 runtime worker 配置，直接 `systemctl restart`

## 容器 / 编排场景建议

如果你在容器里运行：

- 把配置文件作为显式挂载，而不是依赖镜像内默认路径猜测
- `/status`、`/metrics` 和 `/-/config` 仍然要做网段隔离，不要因为“在集群里”就默认公开
- `SIGTERM` 应直接传给主进程，让 `rginx` 完成平滑退出
- 如果做 readiness probe，优先单独配一个静态 `/-/ready` 或 `/healthz`，不要直接把 `/status` 裸暴露给平台探针

## 卸载与回滚

默认卸载行为会：

- 删除 `rginx` 二进制
- 删除卸载脚本自身
- 删除文档和示例配置
- 保留活跃配置目录

这意味着默认卸载更接近“移除程序，保留现场”，适合保留配置做排障或回滚。

如果你明确需要清理所有安装痕迹，再使用：

```bash
rginx-uninstall --purge-config
```

## 推荐阅读

- [Operations](Operations.md)
- [Release Gate](Release-Gate.md)
- [Release Process](Release-Process.md)
- [Examples](Examples.md)
