use super::*;

pub(super) fn role_summary(actor: &AuthenticatedActor) -> String {
    actor.user.roles.first().copied().map(role_label).unwrap_or("管理员").to_string()
}

fn role_label(role: AuthRole) -> &'static str {
    match role {
        AuthRole::SuperAdmin => "管理员",
    }
}

pub(super) fn stream_state_text(state: StreamState) -> &'static str {
    match state {
        StreamState::Idle => "未连接",
        StreamState::Connecting => "连接中",
        StreamState::Live => "在线",
        StreamState::Reconnecting => "重连中",
        StreamState::Error => "异常",
    }
}

pub(super) fn bool_switch_label(value: bool) -> &'static str {
    if value { "已启用" } else { "未启用" }
}

pub(super) fn status_label(value: &str) -> &str {
    match value {
        "super_admin" => "管理员",
        "critical" => "严重",
        "warning" => "提醒",
        "provisioning" => "初始化中",
        "online" => "在线",
        "draining" => "排空中",
        "offline" => "离线",
        "drifted" => "漂移",
        "draft" => "草稿",
        "pending" => "待处理",
        "running" => "进行中",
        "paused" => "已暂停",
        "succeeded" => "已完成",
        "failed" => "失败",
        "rolled_back" => "已回滚",
        "dispatched" => "已派发",
        "acknowledged" => "已确认",
        "cancelled" => "已取消",
        "active" => "生效中",
        "apply_revision" => "下发版本",
        "rollback_revision" => "回滚版本",
        "valid" => "通过",
        "invalid" => "未通过",
        "published" => "已发布",
        "static_ip" => "静态 IP",
        "cluster" => "集群",
        "node" => "节点",
        "upstream" => "上游",
        "static" => "静态值",
        "none" => "未启用",
        "configured" => "已配置",
        "loaded" => "已加载",
        "empty" => "空缓存",
        "ok" => "正常",
        _ => value,
    }
}

pub(super) fn runtime_summary_rows(
    runtime: Option<&RuntimeStatusSnapshot>,
    counters: Option<&HttpCountersSnapshot>,
) -> Vec<(String, String)> {
    let Some(runtime) = runtime else {
        return Vec::new();
    };
    vec![
        ("配置路径".to_string(), runtime.config_path.clone().unwrap_or_else(|| "-".to_string())),
        ("工作线程".to_string(), format_optional(runtime.worker_threads)),
        ("接入线程".to_string(), runtime.accept_workers.to_string()),
        ("监听器".to_string(), runtime.listeners.len().to_string()),
        ("虚拟主机".to_string(), runtime.total_vhosts.to_string()),
        ("路由".to_string(), runtime.total_routes.to_string()),
        ("上游".to_string(), runtime.total_upstreams.to_string()),
        ("活跃连接".to_string(), runtime.active_connections.to_string()),
        ("HTTP/3 活跃连接".to_string(), runtime.http3_active_connections.to_string()),
        (
            "请求数".to_string(),
            counters
                .map(|item| item.downstream_requests.to_string())
                .unwrap_or_else(|| "0".to_string()),
        ),
        (
            "响应数".to_string(),
            counters
                .map(|item| item.downstream_responses.to_string())
                .unwrap_or_else(|| "0".to_string()),
        ),
        ("重载结果".to_string(), format_reload_result(runtime.reload.last_result.as_ref())),
    ]
}

pub(super) fn parse_dns_runtime_status(
    snapshot: Option<&rginx_control_types::NodeSnapshotDetail>,
) -> Option<DnsRuntimeStatus> {
    let dns_value = snapshot
        .and_then(|snapshot| snapshot.status.as_ref())
        .and_then(|status| status.get("dns"))?;

    if dns_value.is_object() {
        return serde_json::from_value(dns_value.clone()).ok();
    }

    dns_value
        .as_array()
        .and_then(|items| items.iter().find_map(|item| serde_json::from_value(item.clone()).ok()))
}

pub(super) fn dns_bind_summary(runtime: Option<&DnsRuntimeStatus>) -> String {
    let Some(runtime) = runtime else {
        return "节点未上报本地权威 DNS 监听".to_string();
    };
    format!(
        "{} · {}",
        runtime.udp_bind_addr.clone().unwrap_or_else(|| "udp -".to_string()),
        runtime.tcp_bind_addr.clone().unwrap_or_else(|| "tcp -".to_string())
    )
}

pub(super) fn dns_runtime_summary_rows(
    runtime: Option<&DnsRuntimeStatus>,
) -> Vec<(String, String)> {
    let Some(runtime) = runtime else {
        return vec![
            ("监听地址".to_string(), "未启用".to_string()),
            ("已发布版本".to_string(), "-".to_string()),
            ("Zone / 记录".to_string(), "0 / 0".to_string()),
            ("热点请求".to_string(), "-".to_string()),
            ("异常请求".to_string(), "-".to_string()),
            ("响应统计".to_string(), "NOERROR 0 · NXDOMAIN 0 · SERVFAIL 0".to_string()),
        ];
    };
    vec![
        (
            "监听地址".to_string(),
            if runtime.enabled { dns_bind_summary(Some(runtime)) } else { "未启用".to_string() },
        ),
        (
            "已发布版本".to_string(),
            runtime.published_revision_id.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("Zone / 记录".to_string(), format!("{} / {}", runtime.zone_count, runtime.record_count)),
        ("查询量".to_string(), runtime.query_total.to_string()),
        (
            "热点请求".to_string(),
            runtime.hot_queries.first().map(dns_query_summary).unwrap_or_else(|| "-".to_string()),
        ),
        (
            "异常请求".to_string(),
            runtime
                .error_queries
                .first()
                .map(dns_query_error_summary)
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "响应统计".to_string(),
            format!(
                "NOERROR {} · NXDOMAIN {} · SERVFAIL {}",
                runtime.response_noerror_total,
                runtime.response_nxdomain_total,
                runtime.response_servfail_total
            ),
        ),
    ]
}

pub(super) fn dns_query_zone_label(query: &DnsRuntimeQueryStat) -> String {
    query.zone_name.clone().unwrap_or_else(|| "未命中 Zone".to_string())
}

pub(super) fn dns_query_error_total(query: &DnsRuntimeQueryStat) -> u64 {
    query.response_nxdomain_total.saturating_add(query.response_servfail_total)
}

fn dns_query_summary(query: &DnsRuntimeQueryStat) -> String {
    format!("{} · 查询 {} · 返回 {}", query.qname, query.query_total, query.answer_total)
}

fn dns_query_error_summary(query: &DnsRuntimeQueryStat) -> String {
    format!("{} · 异常 {}", query.qname, dns_query_error_total(query))
}

fn format_reload_result(result: Option<&crate::runtime::ReloadResultSnapshot>) -> String {
    let Some(result) = result else {
        return "-".to_string();
    };
    if let Some(success) = result
        .outcome
        .get("Success")
        .and_then(|value| value.get("revision"))
        .and_then(|value| value.as_u64())
    {
        return format!("成功 · 版本 {success}");
    }
    if let Some(error) = result
        .outcome
        .get("Failure")
        .and_then(|value| value.get("error"))
        .and_then(|value| value.as_str())
    {
        return format!("失败 · {error}");
    }
    pretty_json(&result.outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rginx_control_types::{AuthSessionSummary, AuthUserSummary, DnsRecordType};

    fn actor_with_roles(roles: Vec<AuthRole>) -> AuthenticatedActor {
        AuthenticatedActor {
            user: AuthUserSummary {
                user_id: "user-1".to_string(),
                username: "console-user".to_string(),
                display_name: "Console User".to_string(),
                active: true,
                roles,
                created_at_unix_ms: 0,
            },
            session: AuthSessionSummary {
                session_id: "sess-1".to_string(),
                issued_at_unix_ms: 0,
                expires_at_unix_ms: 0,
            },
        }
    }

    #[test]
    fn role_and_status_labels_match_console_copy() {
        assert_eq!(role_summary(&actor_with_roles(vec![AuthRole::SuperAdmin])), "管理员");
        assert_eq!(status_label("critical"), "严重");
        assert_eq!(status_label("warning"), "提醒");
        assert_eq!(status_label("super_admin"), "管理员");
        assert_eq!(status_label("unknown"), "unknown");
    }

    #[test]
    fn stream_and_switch_labels_match_css_contract() {
        assert_eq!(stream_state_text(StreamState::Idle), "未连接");
        assert_eq!(stream_state_text(StreamState::Live), "在线");
        assert_eq!(stream_state_text(StreamState::Error), "异常");
        assert_eq!(bool_switch_label(true), "已启用");
        assert_eq!(bool_switch_label(false), "未启用");
    }

    #[test]
    fn dns_error_total_sums_failure_buckets() {
        let query = DnsRuntimeQueryStat {
            zone_name: Some("example.com".to_string()),
            qname: "www.example.com".to_string(),
            record_type: DnsRecordType::A,
            query_total: 7,
            answer_total: 5,
            response_noerror_total: 5,
            response_nxdomain_total: 3,
            response_servfail_total: 2,
            last_query_at_unix_ms: 0,
        };
        assert_eq!(dns_query_error_total(&query), 5);
    }
}
