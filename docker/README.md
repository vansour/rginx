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
```

启动前必须先修改 `.env` 中的敏感默认值，至少包括以下变量：

- `RGINX_CONTROL_AUTH_SESSION_SECRET`
- `RGINX_CONTROL_AGENT_SHARED_TOKEN`
- `RGINX_CONTROL_BOOTSTRAP_ADMIN_PASSWORD`

发布包中的 `compose.yaml` 继承自仓库根目录，默认仍保留本地源码构建配置；如果你直接
使用解压后的发布包启动，请先做两处调整：

- 将 `image: rginx-web:${RGINX_WEB_IMAGE_TAG:-dev}` 改为
  `image: ghcr.io/vansour/rginx-web:<same-release-tag>`
- 删除 `build:` 配置块，因为发布包不包含 `Dockerfile`

完成以上调整后再启动：

```bash
docker compose --env-file .env up -d
```

如果你是在完整源码仓库中使用 `compose.yaml`，则可以保留现有 `build:` 配置并按仓库内
的 Dockerfile 本地构建。

## Runtime Notes

- 发布到 `ghcr.io/vansour/rginx-web:<tag>` 的镜像已经内置
  `/opt/rginx/control-console`，并默认设置了 `RGINX_CONTROL_UI_DIR`
- 打包附带的 `control-console/` 目录主要用于直接运行 `rginx-web` 二进制，或构建未内置
  UI 资源的自定义镜像；只有这些场景才需要手动设置 `RGINX_CONTROL_UI_DIR`
- 发布包中的 `compose.yaml` 会启动控制面所需容器，但不会默认挂载本地
  `control-console/` 目录
- 边缘节点上的 `rginx` 与 `rginx-node-agent` 仍按各自部署方式运行，不包含在该
  Compose 包里
