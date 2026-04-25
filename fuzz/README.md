# rginx Fuzz Targets

当前仓库使用 `cargo-fuzz` 管理 fuzz 入口。

## 安装

```bash
cargo install cargo-fuzz
rustup toolchain install nightly-2026-04-24
```

## 当前目标

- `proxy_protocol`
  - 覆盖 `PROXY protocol v1` 解析
- `config_preprocess`
  - 覆盖配置预处理、环境变量展开和 RON 载入入口
- `ocsp_response`
  - 覆盖 OCSP 响应校验入口
- `certificate_inspect`
  - 覆盖 PEM/DER 证书链载入、证书字段检查和链路诊断
- `ocsp_responder_discovery`
  - 覆盖证书 AIA 扩展里的 OCSP responder URL 发现

## 运行

```bash
cargo fuzz run proxy_protocol
cargo fuzz run config_preprocess
cargo fuzz run ocsp_response
cargo fuzz run certificate_inspect
cargo fuzz run ocsp_responder_discovery
```

仓库根目录还提供一个 seed 刷新脚本：

```bash
./scripts/refresh-fuzz-seeds.sh
```

## Smoke

仓库根目录提供一个短时 smoke 脚本：

```bash
./scripts/run-fuzz-smoke.sh --seconds 10
```

默认只回放版本化 `.seed`。如果要把本地探索积累的完整 corpus 也一起回放：

```bash
./scripts/run-fuzz-smoke.sh --seconds 10 --full-corpus
```

## Coverage

生成文本和 HTML 覆盖率报告：

```bash
./scripts/run-fuzz-coverage.sh --target proxy_protocol
```

默认输出：

- `fuzz/coverage/<target>/report.txt`
- `fuzz/coverage/<target>/html/index.html`

## 说明

- 在 `fuzz/` 目录中执行这些命令。
- `fuzz/rust-toolchain.toml` 会把 `fuzz/` 目录固定到经过验证的 nightly toolchain。
- `fuzz/dictionaries/<target>.dict` 提供 target-specific 字典，`run-fuzz-smoke.sh` 会自动加载。
- `fuzz/options/<target>.options` 提供 target-specific libFuzzer 参数，`run-fuzz-smoke.sh` 和 `run-fuzz-coverage.sh` 会自动加载。
- `fuzz/corpus/<target>/*.seed` 是版本化精选输入，`refresh-fuzz-seeds.sh` 可用于重建这些 seed。
- `run-fuzz-smoke.sh` 和 `run-fuzz-coverage.sh` 默认会把这些 `.seed` 复制到临时目录再回放，避免本地自动生成 corpus 让 smoke 和 coverage 结果漂移。
- 目标优先覆盖高风险输入边界，不承诺每个目标都立即达到深层状态空间。
- `fuzz/target`、`fuzz/artifacts`、`fuzz/coverage` 默认不进版本库。
- `fuzz/corpus/<target>/` 下 libFuzzer 自动发现的非 `.seed` 语料默认被 `.gitignore` 忽略，避免本地探索把工作区刷脏。
