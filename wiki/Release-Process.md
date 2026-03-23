# Release Process

本页把正式版 tag 前后的操作收成一套可执行流程。当前稳定发布目标是 `v0.1.1`。

## 目标

正式版发布的最低要求仍以 [Release Gate](Release-Gate.md) 为准；本页只负责把这些要求展开成明确的操作步骤。

## 发布前提

在切正式版 tag 之前，先确认：

- 目标版本号已经收口，不再带 `-rc`
- 你当前就在 `main`，且本地 `HEAD` 与 `origin/main` 一致
- 工作区没有未提交的临时改动
- GitHub CLI 已登录，具备 push tag 和查看 release 的权限

## 自动化预检

仓库现在提供了一个正式版预检脚本：

```bash
./scripts/prepare-release.sh --tag v0.1.1
```

它会自动检查：

- 当前分支是否为 `main`
- 工作区是否干净
- 本地 `HEAD` 是否等于 `origin/main`
- 目标 tag 是否已经存在于本地或远端
- `Cargo.toml` 里的工作区版本是否与目标 tag 一致
- `cargo fmt --all --check`
- `cargo test --workspace --locked`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo run -p rginx -- --version` 是否输出与 tag 对应的稳定版本

如果你只是离线复核，可以加 `--skip-fetch`；如果你明确知道工作区里有暂存中的发布辅助改动，可以临时加 `--allow-dirty`。

## 手工 Smoke

自动化校验通过后，正式版前至少再做下面这些手工 smoke：

1. 配置检查：

```bash
cargo run -p rginx -- check --config configs/rginx.ron
```

2. 基础代理与状态页：

```bash
cargo run -p rginx -- --config configs/rginx.ron
curl -i http://127.0.0.1:8080/
curl -i http://127.0.0.1:8080/status
curl -i http://127.0.0.1:8080/metrics
```

3. 静态文件、`HEAD` 与单段 `Range`：

```bash
curl -I http://127.0.0.1:8080/static/<file>
curl -i -H 'Range: bytes=0-15' http://127.0.0.1:8080/static/<file>
```

4. TLS / HTTP2、健康检查、热重载和平滑退出：

- 用仓库内对应示例配置验证 TLS 与 ALPN HTTP/2
- 用主备或健康检查示例配置验证 failover
- 发送 `SIGHUP`，确认监听地址不变时热重载成功
- 发送 `SIGTERM` 或 `Ctrl-C`，确认进程平滑退出

5. 安装/卸载链路：

```bash
./scripts/install.sh --mode source --prefix /tmp/rginx-release-smoke --force
/tmp/rginx-release-smoke/bin/rginx check
/tmp/rginx-release-smoke/bin/rginx-uninstall -y
```

这一步的目标不是替代 release artifact 验证，而是确认正式版承诺里的“基础安装体验”在本地仍然成立。

## 切 Tag

当预检和手工 smoke 都通过后，正式版 tag 流程如下：

```bash
git switch main
git pull --ff-only
./scripts/prepare-release.sh --tag v0.1.1
git tag v0.1.1
git push origin v0.1.1
```

说明：

- 正式版 tag 必须直接切在 `origin/main` 当前 HEAD 上
- 不要从功能分支、旧提交或本地未同步的 `main` 上直接打 tag

## 发布后核对

tag push 后，GitHub Actions 的 `Release` workflow 会自动创建或更新同名 Release。至少检查：

- `Release` workflow 全部通过
- 四个平台 archive 都已上传
- `SHA256SUMS.txt` 已生成
- Release notes 包含具体 changelog，而不只是 tag 和 commit
- release archive 内包含：
  - `rginx`
  - `configs/`
  - `scripts/install.sh`
  - `scripts/uninstall.sh`
  - `scripts/prepare-release.sh`
  - `scripts/sync-wiki.sh`
  - `README.md`
  - `CHANGELOG.md`
  - `LICENSE*`

发布后还建议补一轮线上安装验收：

```bash
curl -fsSL https://raw.githubusercontent.com/vansour/rginx/main/scripts/install.sh | bash -s -- --mode release --version v0.1.1
```

如果这是仓库的第一个稳定版，等 `v0.1.1` 发布完成后，`--version latest` 也应开始解析到该稳定 tag。
