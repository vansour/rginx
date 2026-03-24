# rginx Wiki

此目录按本地 wiki 形式组织，入口页见 [Home.md](Home.md)。

如果要同步到 GitHub Wiki 仓库，直接在仓库根目录运行：

```bash
./scripts/sync-wiki.sh
```

同步脚本会把 `wiki/` 下的页面覆盖到 `rginx.wiki.git`，并默认跳过这个仅供仓库内说明使用的 `README.md`。
