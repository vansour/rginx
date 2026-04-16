# Control Plane Deploy Assets

控制面运行入口已经统一收口到仓库根目录的 `compose.yaml` 和 `Dockerfile`。
这个目录不再维护第二套 control-plane runtime 编排，避免和 Docker-only 策略冲突。

当前保留的内容仅用于边缘节点接入控制面时的 APT + systemd 参考：

- `systemd/rginx-node-agent.service`
- `systemd/rginx-node-agent.env.example`

边缘节点通过 systemd 启动 `rginx-node-agent` 时，需要额外提供
`/etc/rginx/rginx-node-agent.env`，至少包含：

```env
RGINX_CONTROL_AGENT_SHARED_TOKEN=replace-me-for-agent
```

建议将该文件权限设置为 `0600`，并按需补充 control-plane origin、节点 ID、cluster、
advertise addr 等覆盖项。
可以直接参考 `systemd/rginx-node-agent.env.example`。

控制面单机部署、环境变量和运维说明请直接查看：

- 根目录 `compose.yaml`
- 根目录 `.env.example`
- 根目录 `Dockerfile`
- `docker/README.md`
