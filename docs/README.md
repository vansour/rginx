# rginx Docs Index

当前仓库的 HTTP/3 与发布收口文档集中在 `docs/` 目录。

## HTTP/3 主线

- `ARCHITECTURE_HTTP3_NGINX_ALIGNMENT_PLAN.md`
  - 下游 HTTP/3 对齐目标、阶段划分、验收口径
- `HTTP3_PHASE0_BASELINE.md`
  - HTTP/3 主线的阶段 0 基线与起始约束
- `HTTP3_PHASE7_RELEASE.md`
  - HTTP/3 发布门禁和 soak 入口

## 上游 HTTP/3 专项

- `ARCHITECTURE_UPSTREAM_HTTP3_PRODUCTION_PLAN.md`
  - 上游 HTTP/3 生产级传输专项计划与阶段收口
- `ARCHITECTURE_UPSTREAM_HTTP3_PHASE0_BASELINE.md`
  - 上游 HTTP/3 专项的阶段 0 基线

## 维护约定

- 如果 README、release notes 或 workflow 引用了新的架构文档，优先把文档放在 `docs/` 下并在这里登记。
- 发布前变更了 HTTP/3 gate 或 soak 时，至少同步更新 `HTTP3_PHASE7_RELEASE.md`。
