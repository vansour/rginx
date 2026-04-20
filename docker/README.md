# Container Packaging

`rginx-web` 的发布包会附带仓库根目录的 `compose.yaml`、`.env.example`、本文件，以及
`control-console/` 前端静态资源目录。

## Included Files

- `compose.yaml`
- `.env.example`
- `docker/README.md`
- `control-console/`

## Quick Start

在解压后的目录中：

```bash
cp .env.example .env
docker compose --env-file .env up -d
```

默认控制面镜像为 `ghcr.io/vansour/rginx-web:<tag>`。

## Runtime Notes

- `compose.yaml` 会启动控制面所需的容器，并将前端静态资源目录挂给 `rginx-web`
- 如果你不使用 Compose，也可以单独运行 `rginx-web`，并将
  `RGINX_CONTROL_UI_DIR` 指向随包附带的 `control-console/` 目录
- 边缘节点上的 `rginx` 与 `rginx-node-agent` 仍按各自部署方式运行，不包含在该
  Compose 包里
