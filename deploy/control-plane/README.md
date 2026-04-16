# Control Plane Deploy Assets

控制面运行入口已经统一收口到仓库根目录的 `compose.yaml` 和 `Dockerfile`。
这个目录不再维护第二套 control-plane runtime 编排，避免和 Docker-only 策略冲突。

当前保留的内容仅用于边缘节点接入控制面时的 APT + systemd 参考：

- `systemd/rginx-node-agent.service`

控制面单机部署、环境变量和运维说明请直接查看：

- 根目录 `compose.yaml`
- 根目录 `.env.example`
- 根目录 `Dockerfile`
- `docker/README.md`
