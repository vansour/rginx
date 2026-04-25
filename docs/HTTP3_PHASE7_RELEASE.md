# HTTP/3 Phase 7 Release Gate

状态：生效中。

Phase 7 的目标不是再增加协议能力，而是把 HTTP/3 变成发布流程中的硬门禁。

## 默认 release gate 组成

`scripts/run-http3-release-gate.sh` 默认串联两段：

1. dedicated HTTP/3 regression gate
2. focused HTTP/3 soak

可选第三段：

3. nginx comparison Docker harness

## 具体入口

### Dedicated Gate

脚本：`scripts/run-http3-gate.sh`

默认覆盖这些测试目标：

- `http3`
- `upstream_http3`
- `grpc_http3`
- `reload`
- `admin`
- `check`
- `ocsp`

### Focused Soak

脚本：`scripts/run-http3-soak.sh`

特点：

- 支持 `--iterations`
- 支持 `--release`
- 支持 Linux `tc netem` 故障注入
- 支持临时 MTU 收缩

说明：

- `--netem-profile` 可选 `none` / `loss` / `reorder` / `jitter`
- 使用 `--netem-profile` 或 `--mtu` 时需要 root、`tc`、`ip`

### Release Wrapper

脚本：`scripts/run-http3-release-gate.sh`

常用命令：

```bash
./scripts/run-http3-release-gate.sh --soak-iterations 1
./scripts/run-http3-release-gate.sh --soak-iterations 3 --release
./scripts/run-http3-release-gate.sh --with-compare --soak-iterations 1
```

## 与仓库发布流程的关系

- `scripts/prepare-release.sh` 会执行：
  - `./scripts/run-http3-release-gate.sh --soak-iterations 1`
- 对预发布 tag，`scripts/prepare-release.sh` 还会执行：
  - `./scripts/run-fuzz-smoke.sh --seconds 10`
- `.github/workflows/release.yml` 的 verify job 也会执行同一条命令
- 对预发布 tag，`.github/workflows/release.yml` 的 verify job 还会执行同一条 fuzz smoke
- `.github/workflows/nightly.yml` 会在每天 03:00 UTC 定时执行：
  - `./scripts/run-http3-release-gate.sh --soak-iterations 3`
  - 同时保留 `workflow_dispatch` 入口，便于手动重跑
  - 手动 dispatch 时可选运行 `./scripts/run-fuzz-smoke.sh --seconds 10`

这意味着 HTTP/3 gate 已经不是“可选专项测试”，而是 release-prep 的正式组成部分；同时 prerelease 路径也开始消费确定性的 fuzz smoke。

## 推荐通过标准

- dedicated gate 全绿
- focused soak 至少跑 1 轮
- 需要发布候选时，建议再跑 `--release`
- 改动 QUIC、HTTP/3 listener、0-RTT、reload/drain 逻辑时，建议增加 netem 或 MTU 场景

## Optional Compare Harness

如果需要把本地发布候选与 nginx 做相对对比，可以加：

```bash
./scripts/run-http3-release-gate.sh \
  --with-compare \
  --soak-iterations 1 \
  --compare-out-dir target/http3-release/nginx-compare
```

注意：

- compare harness 目标是同一环境下的相对对比，不是对外宣传基准
- 当前 harness 会运行 `rginx` 的 HTTP/3 场景
- nginx 一侧在这套 Docker 构建里仍不提供 QUIC/HTTP/3，对应结果会标成 unsupported

## 维护要求

- 变更 HTTP/3 gate 目标时，更新此文档、README 和 release notes。
- 变更 soak 参数或 fault profile 时，更新此文档中的示例命令和前置条件。
