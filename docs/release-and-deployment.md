# Release And Deployment Closure

这份文档把 Week 8 里要求的 release checklist、systemd / supervisor 建议和上线动作收口到一个地方。

## Release Checklist

### 预发布

1. 确认目标提交已经合并到 `origin/main`。
2. 版本号已经更新到目标 prerelease，例如 `0.1.2-rc.8`。
3. 本地执行：

```bash
./scripts/prepare-release.sh --tag v0.1.2-rc.8
```

4. 打 tag 并 push：

```bash
git tag v0.1.2-rc.8
git push origin v0.1.2-rc.8
```

5. 检查 GitHub Actions `Release` workflow：
   - Verify
   - linux-amd64 build
   - linux-arm64 build
   - Publish GitHub Release
6. 检查 GitHub Release 页面：
   - release notes
   - compare link
   - `SHA256SUMS.txt`
   - 两个 Linux 压缩包

### 正式版

正式版比 prerelease 多一个硬约束：

- tag 必须直接切在 `origin/main` 当前 HEAD

本地命令：

```bash
./scripts/prepare-release.sh --tag v0.1.2
git tag v0.1.2
git push origin v0.1.2
```

## 部署前检查

上线前不要只跑 `rginx check`，至少做下面这几步：

1. `rginx check --config /etc/rginx/rginx.ron`
2. 用目标 supervisor 启动实例
3. 本机执行：

```bash
rginx status --config /etc/rginx/rginx.ron
rginx counters --config /etc/rginx/rginx.ron
rginx peers --config /etc/rginx/rginx.ron
```

4. 跑一次 [benchmark 与 soak 基线](./benchmark-and-soak.md)
5. 验证：
   - access log 正常输出
   - admin socket 可读
   - reload / restart 符合预期

## 推荐安装布局

- 二进制：`/usr/sbin/rginx`
- 主配置：`/etc/rginx/rginx.ron`
- 站点片段：`/etc/rginx/conf.d/*.ron`
- pid：`/run/rginx.pid`
- admin socket：`/run/rginx/admin.sock`

## reload 与 restart 的分工

适合 `reload` 的变更：

- 路由
- upstream peer 列表
- health check
- access control
- rate limit
- vhost 级 TLS

必须 `restart` 的变更：

- `listen`
- listener 集合
- `runtime.worker_threads`
- `runtime.accept_workers`

## systemd

建议直接使用仓库里的示例 unit：

- [deploy/systemd/rginx.service](/root/github/rginx/deploy/systemd/rginx.service)

典型命令：

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now rginx
sudo systemctl reload rginx
sudo systemctl restart rginx
sudo systemctl status rginx
```

## supervisor

如果目标环境已经统一使用 supervisor，直接参考：

- [deploy/supervisor/rginx.conf](/root/github/rginx/deploy/supervisor/rginx.conf)

典型命令：

```bash
sudo supervisorctl reread
sudo supervisorctl update
sudo supervisorctl restart rginx
sudo supervisorctl status rginx
```

## 回滚建议

建议每次 release 都保留：

- 上一版 release archive
- 上一版配置快照
- 一份可直接回退的 systemd / supervisor 配置

如果是配置问题，优先：

```bash
rginx -s reload
```

如果是 listener/runtime 结构变化或二进制问题，直接回到上一版 archive 并执行：

```bash
rginx -s restart
```
