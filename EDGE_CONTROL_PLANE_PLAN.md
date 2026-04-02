# 边缘节点主控与观测方案

本文档描述将 `rginx` 放入 CDN 边缘节点体系后的推荐控制与观测架构。

核心原则：

- `rginx` 只做数据面，不直接暴露远程控制接口。
- 边缘节点由本地 `edge-agent` 负责控制、采集和回传。
- 主控端不主动打边缘节点管理接口，而是由边缘节点主动连主控。
- 配置变更通过“写文件 + `check` + reload”生效。
- 状态与流量信息通过本地只读信息源、日志和系统统计回传。

## 总体架构

整个系统拆成三层：

- `rginx`：纯数据面，只负责转发流量。
- `edge-agent`：节点本地代理，负责收配置、写文件、执行 `check/reload`、采集本地状态、上报遥测。
- `control-plane`：主控端，负责配置编排、发布、回滚、节点管理、指标聚合、日志汇总。

关键设计：

- 主控端不直接访问公网边缘节点的管理接口。
- 由边缘节点上的 `edge-agent` 主动向主控端建立长期 `mTLS` 连接。
- 所有配置控制和状态回传，都走这条由边缘节点主动发起的出站连接。

## 一、主控端如何控制 `rginx` 配置

推荐控制链路：

1. 主控端维护“期望配置”

- 不直接保存边缘节点的本地文件，而是保存更高层的 CDN 配置模型。
- 例如：域名、证书、回源、ACL、限速、健康检查、节点分组、灰度策略。

2. 主控端编译出“节点配置快照”

- 按节点、区域、机房、业务组生成最终下发的配置。
- 每个快照带：
  - `revision`
  - `created_at`
  - `sha256`
  - 可选签名
  - 变更说明

3. `edge-agent` 主动拉取或订阅配置

- 最好是长连接流式订阅，而不是频繁轮询。
- 边缘节点连上主控后，声明：
  - 节点 ID
  - 区域/机房
  - 当前 revision
  - 能力标签
  - 健康状态

4. `edge-agent` 本地落盘并验证

- 把新配置写到临时文件。
- 执行：
  - `rginx check --config <temp>`
- 校验通过后，原子替换正式配置文件。
- 然后执行：
  - `rginx -s reload`
  - 或直接发 `SIGHUP`

5. `edge-agent` 回报结果

- 成功：上报 `applied_revision`
- 失败：上报错误文本、校验错误、reload 错误
- 必要时自动回滚到上一个已知可用 revision

这套模型里，`rginx` 本身不需要暴露远程写配置 API。

## 二、主控端如何读取边缘节点的速率、流量、状态

不要只依赖一类数据源，应拆成三条链路。

### 1. 快速指标链路

用于看板、报警、自动扩缩容、发布观察。

由 `edge-agent` 周期性上报聚合指标，建议 1 秒采样、5 秒上报一次。

建议采集四类指标：

- 节点级
  - 入站带宽 `ingress_bps`
  - 出站带宽 `egress_bps`
  - 包速率 `pps`
  - CPU、内存、磁盘
  - 网卡丢包、socket 错误
  - 当前连接数

- `rginx` 进程级
  - 活跃连接数
  - 接收请求数/QPS
  - 响应状态分布 `2xx/3xx/4xx/5xx`
  - TLS 握手数/失败数
  - reload 成功/失败次数
  - upstream 超时、失败、failover 次数

- 业务级
  - 每域名 `QPS`
  - 每域名 `BPS`
  - 每域名状态码分布
  - 每上游集群流量
  - 每上游错误率
  - 将来如果做缓存，再加命中率

- 发布级
  - 当前配置 revision
  - 最近一次发布时间
  - 当前生效证书版本
  - 节点是否 drift

### 2. 事件链路

用于记录“发生了什么”，不适合靠指标推导。

事件包括：

- 配置下发成功/失败
- reload 成功/失败
- 节点上线/离线
- 上游集群异常
- 某节点流量突增
- 证书更新成功/失败
- 限流策略变更
- 人工干预操作

### 3. 日志链路

用于审计、追查、计费、离线分析。

主要包括：

- access log
- error log
- reload/restart 日志
- `edge-agent` 操作日志

日志适合计算：

- 精确流量账单
- Top 域名 / Top 回源 / Top URL
- 长尾错误
- 用户行为分析
- 某时段重放排查

三条链路分别解决：

- 指标看“现在”
- 事件看“发生了什么”
- 日志看“细节和历史”

## 三、如果不开放 HTTP 管理路由，节点本地该怎么采集

既然收口为“不要公开管理路由”，就应让 `edge-agent` 只读本地接口。

推荐两层方案。

### 方案 A：先用现有能力，最快落地

`edge-agent` 先不依赖 `rginx` 新接口，只做这些事情：

- 写配置文件
- 调 `rginx check`
- 发 reload 信号
- 读 `rginx` 日志
- 读进程信息 `/proc`
- 读网卡统计 `/sys/class/net/*/statistics`
- 读 systemd/journal 或本地日志文件

这能马上拿到：

- 节点带宽
- 进程状态
- reload 成败
- 基础业务量
- 错误趋势

缺点：

- 业务级指标不够精细
- 很多统计需要靠日志再聚合

### 方案 B：给 `rginx` 增加“仅本地可访问”的状态输出

这是更推荐的正式形态，但仍然不走公网 HTTP。

推荐两种本地接口形式，二选一：

- Unix Domain Socket
- `/run/rginx/*.json` 或 `/run/rginx/*.prom` 文件

更推荐 `UDS`：

- 可按文件权限限制只有 `edge-agent` 可读
- 不占 TCP 端口
- 不会被公网访问
- 支持按需查询

例如：

- `/run/rginx/admin.sock`

提供只读接口：

- `GetStatus`
- `GetCounters`
- `GetPeerHealth`
- `GetRevision`

如果要极简，也可以先做状态文件：

- `/run/rginx/status.json`
- `/run/rginx/counters.json`

由 `rginx` 每秒刷新，`edge-agent` 周期读取。

文件方式优点：

- 实现简单
- 出问题时人工可直接 `cat`

缺点：

- 实时性和并发语义较弱
- 数据结构扩展不如 socket 灵活

## 四、配置控制和状态回传的推荐协议

推荐控制和状态两条通道都走 `gRPC over mTLS`。

### 控制通道

- 长连接
- 边缘节点主动连接主控
- 主控下发：
  - 目标 revision
  - 配置快照
  - 证书材料或证书引用
  - 命令：reload、drain、freeze、resume

### 状态通道

- 可以复用同一条流，也可以独立
- 边缘节点上报：
  - 心跳
  - 当前 revision
  - 应用结果
  - 指标摘要
  - 事件
  - 健康状态

如果规模不大，控制和状态可以一条流完成。

如果规模大，再拆成：

- 控制流
- 指标流
- 日志流

## 五、边缘节点配置发布的正式流程

正式方案一定要有 revision、灰度、回滚。

推荐流程：

1. 主控生成 `revision N`
2. 选择一小批 canary 节点
3. `edge-agent` 下载配置
4. 本地 `check`
5. 原子替换
6. reload
7. 等待 30 秒到 5 分钟观察窗口
8. 观察这些信号：
   - reload 成功率
   - 节点存活率
   - QPS/BPS 是否异常
   - 5xx 是否上升
   - upstream timeout 是否飙升
   - TLS 握手失败是否上升
9. 若正常，再扩大批次
10. 若异常，主控发回滚命令，节点恢复到 `revision N-1`

不要做“直接全网推送后相信一切正常”。

## 六、边缘节点应该上报哪些最重要的字段

建议先从下面这组最小字段集合开始。

### 节点心跳

- `node_id`
- `region`
- `az` / `pop`
- `public_ip`
- `version`
- `uptime`
- `current_revision`
- `last_apply_status`
- `last_apply_time`

### 节点负载

- `cpu_percent`
- `mem_percent`
- `disk_percent`
- `ingress_bps`
- `egress_bps`
- `active_connections`

### 业务汇总

- `requests_per_sec`
- `responses_2xx_per_sec`
- `responses_4xx_per_sec`
- `responses_5xx_per_sec`
- `bytes_in_per_sec`
- `bytes_out_per_sec`
- `tls_handshakes_per_sec`
- `upstream_timeout_per_sec`
- `upstream_failover_per_sec`

### 按域名聚合

- `host`
- `qps`
- `bps_in`
- `bps_out`
- `4xx_rate`
- `5xx_rate`

不要一开始就做太细的 per-path 高频指标，避免边缘端和主控端被标签爆炸拖慢。

## 七、关于“流量、速率”的准确获取方式

需要区分不同统计口径。

### 1. 节点出口/入口带宽

最准确的是网卡层统计：

- `/sys/class/net/<iface>/statistics/rx_bytes`
- `/sys/class/net/<iface>/statistics/tx_bytes`

适合看：

- 物理带宽
- 节点总吞吐
- 粗粒度计费

### 2. 代理业务流量

更适合从 `rginx` 内部计数获得：

- 请求体字节
- 响应体字节
- 域名维度 bytes
- upstream bytes

适合看：

- 业务流量
- 回源流量
- 域名流量

### 3. 精确审计/计费

最终仍建议以 access log 为准做离线归并。

因为以下场景很难完全靠单一实时计数正确表达：

- chunked
- upgrade
- gRPC streaming
- 提前中断
- 重试/回源失败

## 八、对 `rginx` 本身的建议

如果要把它放进 CDN 边缘体系，不建议让 `rginx` 自己承担“远程控制面协议”，而是只承担“本地数据面 + 本地只读状态”。

也就是说：

- `rginx` 负责：
  - 流量转发
  - 本地配置加载
  - reload
  - 本地计数器
  - 本地状态输出
  - 日志

- `edge-agent` 负责：
  - 和主控通信
  - 拉配置
  - 写文件
  - 调 `check`
  - 触发 reload
  - 采集本地状态
  - 聚合与上报
  - 回滚

这样边界最清楚，后续演进也最稳。

## 九、最推荐的落地路径

如果现在开始做，建议按成熟度分三阶段推进。

### 第一阶段，最快能上线

- 不做公网管理路由
- 做 `edge-agent`
- 配置下发采用“边缘主动长连接 + 文件落盘 + check + reload”
- 指标先采：
  - 网卡流量
  - 进程状态
  - access log 聚合
  - reload 成败
- 日志统一回传

### 第二阶段，补本地只读接口

- 给 `rginx` 增加本地 `UDS` 或 `/run/rginx/status.json`
- 让 `edge-agent` 直接读内部 counters
- 开始做 per-host QPS/BPS、upstream error、active connections

### 第三阶段，做成熟控制面

- 灰度发布
- 分批 rollout
- revision / rollback
- 证书下发
- 节点分组
- 变更审计
- 观测闭环

## 一句话总结

如果要把 `rginx` 放进 CDN 边缘体系，最合适的方式不是“给它开放远程管理路由”，而是：

- `rginx` 做纯数据面
- `edge-agent` 做本地控制与采集
- 边缘节点主动连主控
- 配置通过文件和 reload 生效
- 状态和流量通过本地只读接口、日志和系统统计回传
