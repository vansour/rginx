# Changelog

`rginx` 目前以 GitHub Releases 作为正式发布线的权威 changelog 来源，而不是在仓库内手工维护一份持续追加的版本日志。

规则如下：

1. 每个发布 tag 都由 `.github/workflows/release.yml` 触发 release workflow。
2. workflow 会在 Release notes 中写入当前 tag 的 commit、上一个 tag、compare 链接和本次发布的 `## Changelog`。
3. 预发布和正式版都遵循同一套 changelog 生成逻辑。

查看方式：

- Releases 页面：<https://github.com/vansour/rginx/releases>
- 发布 workflow：[`/.github/workflows/release.yml`](./.github/workflows/release.yml)

如果仓库根目录的文档、wiki 和 GitHub Release notes 出现发布内容差异，应以对应 tag 的 GitHub Release notes 为准。
