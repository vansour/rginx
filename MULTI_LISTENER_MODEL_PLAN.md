# 多监听入口模型设计草案

这份文档是 `rginx` 在“专项反向代理替代 nginx”路线里的 Week 4 设计交付物。

它解决的是“如何把当前单实例单 `listen` 的配置模型，升级成可覆盖典型 nginx 反向代理入口形态”的问题。

本文只定义：

- 配置模型的目标形态
- `listener` 与 `vhost` 的职责边界
- 旧配置到新配置的兼容编译路径
- 迁移示例
- Week 5 的实现落点

本文不直接引入运行时实现；真正的多 listener 编译和 accept loop 改造属于 Week 5。

## 一、为什么现在必须做这件事

当前 `rginx` 仍围绕一个 `server.listen` 工作。这对“单站点、单入口”的场景足够，但它会直接卡住下面这些非常常见的反向代理部署：

- `:80` 做 HTTP 明文入口，统一 301/308 到 `:443`
- `:443` 做 TLS 终止和 HTTP/2 / gRPC 入口
- IPv4 / IPv6 双入口
- 同一个进程同时承接内网和公网入口
- 同一个站点在多个监听地址上暴露，但路由和 vhost 语义保持一致

如果不先把配置模型拆清，后续无论是多入口实现、优雅重启还是更细的 listener 级治理，都会继续被当前 `server` 结构绑死。

## 二、设计目标

目标只围绕专项反向代理场景，不追求把配置 DSL 做成 nginx 的镜像。

本次设计要满足：

- 能表达多个监听入口
- 能表达同一进程同时监听 `:80` 和 `:443`
- 能表达 listener 级 TLS 和连接治理
- 能保持 vhost / route 的现有语义稳定
- 能让旧配置通过兼容路径继续工作
- 能为 Week 5 的实现提供明确落点，而不是继续讨论抽象

## 三、明确非目标

这次设计不解决下面这些问题：

- 多 listener 的真正运行时实现
- 热重载切换 listener
- 优雅重启 / fd 继承
- HTTP/3
- 更复杂的 listener 级 ACL、限流、证书路由语法糖
- 完整 nginx `listen` 指令兼容

## 四、核心思路

把当前的顶层 `server` 从“默认虚拟主机 + 监听参数混合体”拆成两层：

- `listeners: []`
  - 描述“这个进程在哪些入口上接受连接”
- `server` + `servers`
  - 继续描述“默认 vhost 和额外 vhost 的 Host/Path/gRPC 路由语义”

也就是说，监听入口和虚拟主机不再是同一个配置对象。

## 五、建议模型

建议在顶层 `Config` 上新增：

```ron
Config(
    runtime: RuntimeConfig(...),
    listeners: [
        ListenerConfig(
            name: "http",
            listen: "0.0.0.0:80",
            trusted_proxies: [],
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        ),
        ListenerConfig(
            name: "https",
            listen: "0.0.0.0:443",
            trusted_proxies: [],
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: Some(ServerTlsConfig(
                cert_path: "/etc/rginx/certs/default.crt",
                key_path: "/etc/rginx/certs/default.key",
            )),
        ),
    ],
    server: ServerConfig(
        server_names: ["example.com", "www.example.com"],
    ),
    upstreams: [...],
    locations: [...],
    servers: [...],
)
```

对应新增：

```rust
pub struct ListenerConfig {
    pub name: String,
    pub listen: String,
    pub trusted_proxies: Vec<String>,
    pub keep_alive: Option<bool>,
    pub max_headers: Option<u64>,
    pub max_request_body_bytes: Option<u64>,
    pub max_connections: Option<u64>,
    pub header_read_timeout_secs: Option<u64>,
    pub request_body_read_timeout_secs: Option<u64>,
    pub response_write_timeout_secs: Option<u64>,
    pub access_log_format: Option<String>,
    pub tls: Option<ServerTlsConfig>,
}
```

同时把当前顶层 `server` 收口为“默认虚拟主机配置”：

```rust
pub struct ServerConfig {
    pub server_names: Vec<String>,
}
```

注意：这里只是目标模型。Week 5 真正实现时，运行时快照里也应引入对应的 `Listener` 结构。

## 六、字段职责边界

### 1. listener 负责什么

listener 只负责“入口连接层和下游连接治理”：

- `listen`
- `trusted_proxies`
- `keep_alive`
- `max_headers`
- `max_request_body_bytes`
- `max_connections`
- `header_read_timeout_secs`
- `request_body_read_timeout_secs`
- `response_write_timeout_secs`
- `access_log_format`
- `tls`

这些字段的共同特点是：它们决定的是“这个 socket / 这个入口怎么接连接、怎么约束下游、怎么做 TLS 和日志”。

### 2. vhost 负责什么

默认 `server` 和额外 `servers: []` 继续负责“Host/Path 路由和上游动作”：

- `server_names`
- `locations`
- vhost 级可选 TLS 覆盖（保留现有能力）

这些字段的共同特点是：它们决定的是“请求进来后如何按 Host/Path/gRPC service+method 选路”。

### 3. 为什么不把路由直接挂到 listener

因为当前 `rginx` 的产品边界还是 L7 虚拟主机 + 路由，而不是“每个监听入口是一套独立站点树”。如果把路由也下放到 listener，会出现：

- 同一个站点在 `:80` 和 `:443` 上需要重复定义 routes
- vhost 和 route 优先级逻辑被监听层污染
- 未来做 HTTP `:80 -> :443` redirect 时反而更绕

所以 listener 只做入口层，vhost 继续做站点层。

## 七、TLS 归属策略

### 1. 默认 listener TLS

listener 可以携带一个默认 TLS 配置，作为该 listener 上的兜底证书来源。

### 2. vhost TLS 覆盖

额外 `servers: []` 里的 vhost 继续允许声明自己的 TLS 证书。Week 5 实现时，应保证：

- vhost 自己有证书时，优先使用 vhost 证书参与 SNI 选择
- listener 的 TLS 证书只做该 listener 的默认兜底

### 3. 不在 Week 4 解决的点

本设计不在 Week 4 解决“同名 vhost 在多个 listener 上绑定不同证书”的更复杂语义。Week 5 第一版只需要先保证：

- 同一个 listener 下的 SNI 选择逻辑稳定
- 多 listener 之间互不干扰

## 八、旧配置兼容编译路径

这是 Week 4 最关键的部分之一。不能因为引入 `listeners: []` 就把现有用户全打断。

兼容策略建议如下：

### 1. 配置层兼容

顶层 `Config` 在过渡期同时接受：

- 旧模型：
  - `server.listen`
  - `server.trusted_proxies`
  - `server.keep_alive`
  - 其他现有 listener 风格字段
- 新模型：
  - `listeners: []`
  - `server.server_names`

### 2. 编译层兼容

如果 `listeners` 为空，则按旧模型自动编译出一个默认 listener：

```text
listeners == []
=> compile one implicit listener from legacy server.listen + server-level connection fields
```

如果 `listeners` 非空，则：

- 旧 `server.listen` 风格字段必须为空或直接报配置错误
- 默认 `server` 只承担 vhost 语义

### 3. 校验层兼容

过渡期校验建议：

- 允许旧模型单独存在
- 允许新模型单独存在
- 禁止“旧 listener 字段 + 新 listeners 同时混写”

这样可以避免用户得到一个语义混合、难以解释的配置。

### 4. 迁移阶段建议

建议分两步：

1. Week 5 到后续一个版本线：
   - 同时支持旧模型和新模型
   - README 把 `listeners: []` 作为推荐写法
2. 再后续版本线：
   - 如果需要，再决定是否正式废弃旧 `server.listen`

当前不建议在 Week 5 直接删掉旧模型。

## 九、迁移示例

### 1. 旧单 listener 模型

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:443",
        server_names: ["example.com"],
        trusted_proxies: ["10.0.0.0/8"],
        tls: Some(ServerTlsConfig(
            cert_path: "/etc/rginx/certs/default.crt",
            key_path: "/etc/rginx/certs/default.key",
        )),
    ),
    upstreams: [...],
    locations: [...],
    servers: [...],
)
```

### 2. 新多 listener 模型

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    listeners: [
        ListenerConfig(
            name: "http",
            listen: "0.0.0.0:80",
            trusted_proxies: ["10.0.0.0/8"],
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        ),
        ListenerConfig(
            name: "https",
            listen: "0.0.0.0:443",
            trusted_proxies: ["10.0.0.0/8"],
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: Some(ServerTlsConfig(
                cert_path: "/etc/rginx/certs/default.crt",
                key_path: "/etc/rginx/certs/default.key",
            )),
        ),
    ],
    server: ServerConfig(
        server_names: ["example.com"],
    ),
    upstreams: [...],
    locations: [...],
    servers: [...],
)
```

### 3. 典型 `:80 -> :443` redirect 形态

Week 5 第一版实现时，推荐示例应长这样：

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    listeners: [
        ListenerConfig(
            name: "http",
            listen: "0.0.0.0:80",
            trusted_proxies: [],
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        ),
        ListenerConfig(
            name: "https",
            listen: "0.0.0.0:443",
            trusted_proxies: [],
            keep_alive: Some(true),
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: Some(ServerTlsConfig(
                cert_path: "/etc/rginx/certs/default.crt",
                key_path: "/etc/rginx/certs/default.key",
            )),
        ),
    ],
    server: ServerConfig(
        server_names: ["example.com", "www.example.com"],
    ),
    upstreams: [...],
    locations: [...],
)
```

这里不要求 `:80` 和 `:443` 有两套独立 routes；如果需要 listener 级 redirect，Week 5 可以通过更轻量的 listener 级“入口动作”或“默认 return policy”补进去，但不应反向污染 vhost 模型。

## 十、Week 5 的明确实现落点

Week 5 真正实现时，建议按这个顺序改：

1. `rginx-config/src/model.rs`
   - 新增 `ListenerConfig`
   - 让顶层 `Config` 接受 `listeners: []`

2. `rginx-config/src/validate.rs`
   - 补 listener 级校验
   - 增加“旧 listener 字段与新 listeners 混写”错误

3. `rginx-config/src/compile.rs`
   - 引入 `CompiledListener`
   - 增加“旧配置 -> implicit default listener”的兼容编译路径

4. `rginx-core`
   - 在运行时快照里引入 `Listener` 模型

5. `rginx-runtime/src/bootstrap.rs`
   - 改成按 listener 集合启动 accept worker 组

6. `rginx-http/src/server.rs`
   - 接受 per-listener 配置，而不是只从全局 `server` 取监听参数

7. 集成测试
   - `:80 + :443`
   - 双 listener
   - IPv4 / IPv6
   - 旧配置兼容路径

## 十一、最终验收标准

Week 4 完成的标志不是“已经支持多 listener”，而是：

- 新模型已经定义清楚
- listener 与 vhost 职责边界已经写清
- 旧配置兼容编译路径已经定稿
- 至少有一组清晰迁移示例
- Week 5 的代码落点已经可以直接开工

如果后续实现阶段还需要重新争论“listener 到底负责什么、旧配置怎么兼容”，说明 Week 4 的设计还不算完成。
