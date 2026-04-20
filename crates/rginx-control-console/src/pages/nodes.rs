use super::*;

#[component]
pub fn EdgeNodes() -> Element {
    let session = use_session();
    let actor = (session.actor)();
    let mut nodes = use_signal(Vec::<NodeSummary>::new);
    let mut loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let mut reload_serial = use_signal(|| 0_u64);
    let reload_tick = reload_serial();

    let actor_snapshot = actor.clone();
    use_effect(use_reactive!(|(actor_snapshot, reload_tick)| {
        let _ = reload_tick;
        if actor_snapshot.is_none() {
            nodes.set(Vec::new());
            loading.set(false);
            return;
        }
        to_owned![session, nodes, loading, error];
        spawn(async move {
            loading.set(true);
            error.set(None);
            match api::get_nodes().await {
                Ok(list) => nodes.set(list),
                Err(load_error) => {
                    if handle_api_auth_error(&load_error, session) {
                        loading.set(false);
                        return;
                    }
                    error.set(Some(load_error.to_string()));
                }
            }
            loading.set(false);
        });
    }));

    let node_rows = nodes();
    let online = node_rows
        .iter()
        .filter(|node| matches!(node.state, rginx_control_types::NodeLifecycleState::Online))
        .count();
    let offline = node_rows
        .iter()
        .filter(|node| matches!(node.state, rginx_control_types::NodeLifecycleState::Offline))
        .count();
    let draining = node_rows
        .iter()
        .filter(|node| matches!(node.state, rginx_control_types::NodeLifecycleState::Draining))
        .count();
    let drifted = node_rows
        .iter()
        .filter(|node| matches!(node.state, rginx_control_types::NodeLifecycleState::Drifted))
        .count();

    rsx! {
        section { class: "page-shell page-shell--list",
            header { class: "hero",
                div {
                    p { class: "eyebrow", "节点" }
                    h1 { "边缘节点概览" }
                    p { class: "hero-copy", "按节点查看在线状态、运行版本、连接数和诊断入口，便于快速定位异常节点并进入详情页处理。" }
                }
                div { class: "hero-meta",
                    p { strong { "当前用户" } " {actor.as_ref().map(|item| item.user.username.as_str()).unwrap_or(\"-\")}" }
                    p { strong { "访问身份" } " {actor.as_ref().map(role_summary).unwrap_or_else(|| \"-\".to_string())}" }
                    p { strong { "节点数量" } " {node_rows.len()}" }
                    p { strong { "在线节点" } " {online}" }
                }
            }

            if actor.is_none() {
                AuthRequired { message: "登录后才能查看节点列表与诊断入口。" }
            } else {
                section { class: "toolbar",
                    div { class: "toolbar-links",
                        button {
                            class: "secondary-button",
                            onclick: move |_| reload_serial += 1,
                            "刷新节点列表"
                        }
                    }
                    div { class: "identity-card",
                        p { class: "identity-card__name", "{actor.as_ref().map(|item| item.user.display_name.as_str()).unwrap_or(\"访客\")}" }
                        p { class: "identity-card__meta", "集中查看节点状态、版本与诊断入口" }
                    }
                }

                if loading() {
                    StateBanner { tone: "info", message: "正在同步节点状态…" }
                }
                if let Some(message) = error() {
                    StateBanner { tone: "error", message }
                }

                section { class: "metric-grid",
                    MetricCard { title: "节点总数".to_string(), value: node_rows.len().to_string(), description: "当前纳管的全部边缘节点".to_string() }
                    MetricCard { title: "在线节点".to_string(), value: online.to_string(), description: "最近心跳仍在阈值内".to_string() }
                    MetricCard { title: "离线节点".to_string(), value: offline.to_string(), description: "超过阈值未继续上报".to_string() }
                    MetricCard { title: "漂移/排空".to_string(), value: format!("{}/{}", drifted, draining), description: "漂移节点 / 排空中节点".to_string() }
                }

                article { class: "panel",
                    header { class: "panel__header",
                        h2 { "节点列表" }
                        span { "{node_rows.len()} 条" }
                    }
                    if node_rows.is_empty() {
                        p { class: "empty-state", "当前没有节点数据。" }
                    } else {
                        div { class: "table-scroll",
                            table { class: "data-table",
                                thead {
                                    tr {
                                        th { "节点" }
                                        th { "集群" }
                                        th { "状态" }
                                        th { "角色" }
                                        th { "版本" }
                                        th { "连接" }
                                        th { "快照" }
                                        th { "诊断" }
                                    }
                                }
                                tbody {
                                    for node in node_rows {
                                        tr { key: "{node.node_id}",
                                            td {
                                                strong { "{node.node_id}" }
                                                div { class: "cell-meta", "{node.advertise_addr}" }
                                            }
                                            td { "{node.cluster_id}" }
                                            td {
                                                span { class: format!("state-pill state-pill--{}", node.state.as_str()), "{status_label(node.state.as_str())}" }
                                                if let Some(reason) = node.status_reason.as_ref() {
                                                    div { class: "cell-meta", "{reason}" }
                                                }
                                            }
                                            td { "{node.role}" }
                                            td { "{node.running_version}" }
                                            td { "{format_optional(node.active_connections)}" }
                                            td { "{format_optional(node.last_snapshot_version)}" }
                                            td {
                                                div { class: "inline-actions",
                                                    Link { class: "secondary-button secondary-button--link", to: Route::NodeDetail { node_id: node.node_id.clone() }, "详情" }
                                                    Link { class: "secondary-button secondary-button--link", to: Route::NodeTls { node_id: node.node_id.clone() }, "TLS" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn NodeDetail(node_id: String) -> Element {
    let session = use_session();
    let actor = (session.actor)();
    let mut detail = use_signal(|| None::<NodeDetailResponse>);
    let mut loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let stream_state = use_signal(StreamState::default);
    let mut reload_serial = use_signal(|| 0_u64);
    let stream = use_signal(|| None::<EventStream>);
    let reload_tick = reload_serial();

    use_drop(move || close_event_stream(stream));

    let actor_snapshot = actor.clone();
    let node_id_for_effect = node_id.clone();
    use_effect(use_reactive!(|(actor_snapshot, node_id_for_effect, reload_tick)| {
        let _ = reload_tick;
        close_event_stream(stream);
        if actor_snapshot.is_none() {
            detail.set(None);
            loading.set(false);
            return;
        }
        to_owned![session, detail, loading, error, stream_state, stream];
        let current_node_id = node_id_for_effect.clone();
        spawn(async move {
            loading.set(true);
            error.set(None);
            match api::get_node_detail(&current_node_id).await {
                Ok(value) => {
                    detail.set(Some(value));
                    stream_state.set(StreamState::Connecting);
                    match api::ensure_events_session().await {
                        Ok(()) => {
                            match build_node_stream(&current_node_id, detail, error, stream_state) {
                                Ok(handle) => stream.set(Some(handle)),
                                Err(stream_error) => {
                                    error.set(Some(stream_error.to_string()));
                                    stream_state.set(StreamState::Error);
                                }
                            }
                        }
                        Err(stream_error) => {
                            if handle_api_auth_error(&stream_error, session) {
                                loading.set(false);
                                return;
                            }
                            error.set(Some(stream_error.to_string()));
                            stream_state.set(StreamState::Error);
                        }
                    }
                }
                Err(load_error) => {
                    if handle_api_auth_error(&load_error, session) {
                        loading.set(false);
                        return;
                    }
                    error.set(Some(load_error.to_string()));
                }
            }
            loading.set(false);
        });
    }));

    let detail_value = detail();
    let snapshot = detail_value.as_ref().and_then(|detail| detail.latest_snapshot.clone());
    let runtime = snapshot.as_ref().and_then(|snapshot| parse_runtime(Some(snapshot)));
    let counters = snapshot.as_ref().and_then(|snapshot| parse_counters(Some(snapshot)));
    let traffic = snapshot.as_ref().and_then(|snapshot| parse_traffic(Some(snapshot)));
    let dns_runtime =
        snapshot.as_ref().and_then(|snapshot| parse_dns_runtime_status(Some(snapshot)));
    let dns_hot_queries =
        dns_runtime.as_ref().map(|runtime| runtime.hot_queries.clone()).unwrap_or_default();
    let dns_error_queries =
        dns_runtime.as_ref().map(|runtime| runtime.error_queries.clone()).unwrap_or_default();
    let upstream_health =
        snapshot.as_ref().map(|snapshot| parse_upstream_health(Some(snapshot))).unwrap_or_default();
    let upstream_stats =
        snapshot.as_ref().map(|snapshot| parse_upstream_stats(Some(snapshot))).unwrap_or_default();
    let detail_title = detail_value
        .as_ref()
        .map(|item| item.node.node_id.clone())
        .unwrap_or_else(|| node_id.clone());
    let detail_cluster_label = detail_value
        .as_ref()
        .map(|item| item.node.cluster_id.clone())
        .unwrap_or_else(|| "-".to_string());
    let detail_user_label =
        actor.as_ref().map(|item| item.user.username.clone()).unwrap_or_else(|| "-".to_string());
    let detail_last_seen_label = detail_value
        .as_ref()
        .map(|item| format_unix_ms(Some(item.node.last_seen_unix_ms)))
        .unwrap_or_else(|| "-".to_string());
    let detail_addr_label = detail_value
        .as_ref()
        .map(|item| item.node.advertise_addr.clone())
        .unwrap_or_else(|| "-".to_string());
    let detail_identity_name = actor
        .as_ref()
        .map(|item| item.user.display_name.clone())
        .unwrap_or_else(|| "访客".to_string());
    let detail_identity_roles = actor.as_ref().map(role_summary).unwrap_or_else(|| "-".to_string());
    let detail_error_tone = if detail_value.is_some() { "warn" } else { "error" };

    rsx! {
        section { class: "page-shell page-shell--detail",
            header { class: "hero",
                div {
                    p { class: "eyebrow", "节点详情" }
                    div { class: "breadcrumb-row",
                        Link { class: "breadcrumb-link", to: Route::Dashboard {}, "总览" }
                        span { "/" }
                        Link { class: "breadcrumb-link", to: Route::EdgeNodes {}, "节点" }
                    }
                    h1 { {detail_title.clone()} }
                    p { class: "hero-copy", "集中查看该节点的运行状态、监听信息、DNS 摘要与最近操作记录，页面会自动刷新最新数据。" }
                }
                div { class: "hero-meta",
                    p { strong { "所属集群" } span { {detail_cluster_label.clone()} } }
                    p { strong { "节点状态" } " "
                        if let Some(detail) = detail_value.as_ref() {
                            span { class: format!("state-pill state-pill--{}", detail.node.state.as_str()), "{status_label(detail.node.state.as_str())}" }
                        } else {
                            span { "-" }
                        }
                    }
                    p { strong { "实时流" } " "
                        span { class: format!("realtime-pill realtime-pill--{}", stream_state_label(stream_state())), "{stream_state_text(stream_state())}" }
                    }
                    p { strong { "当前用户" } span { {detail_user_label.clone()} } }
                    p { strong { "最近在线" } span { {detail_last_seen_label.clone()} } }
                    p { strong { "节点地址" } span { {detail_addr_label.clone()} } }
                }
            }

            if actor.is_none() {
                AuthRequired { message: "登录后才能查看节点详情。" }
            } else {
                section { class: "toolbar",
                    div { class: "toolbar-links",
                        Link { class: "secondary-button secondary-button--link", to: Route::Dashboard {}, "总览" }
                        Link { class: "secondary-button secondary-button--link", to: Route::NodeTls { node_id: node_id.clone() }, "证书与 TLS" }
                        button {
                            class: "secondary-button",
                            onclick: move |_| reload_serial += 1,
                            "刷新详情"
                        }
                    }
                    div { class: "identity-card",
                        p { class: "identity-card__name", {detail_identity_name.clone()} }
                        p { class: "identity-card__meta", {detail_identity_roles.clone()} }
                    }
                }

                if loading() {
                    StateBanner { tone: "info", message: "正在加载节点详情…" }
                }
                if let Some(message) = error() {
                    StateBanner { tone: detail_error_tone, message }
                }

                if let Some(detail) = detail_value.as_ref() {
                    section { class: "metric-grid",
                        MetricCard {
                            title: "节点状态".to_string(),
                            value: detail.node.state.as_str().to_string(),
                            description: detail.node.status_reason.clone().unwrap_or_else(|| "节点生命周期状态".to_string())
                        }
                        MetricCard {
                            title: "快照版本".to_string(),
                            value: snapshot.as_ref().map(|item| item.snapshot_version.to_string()).unwrap_or_else(|| "-".to_string()),
                            description: snapshot
                                .as_ref()
                                .map(|item| format!("采集于 {}", format_unix_ms(Some(item.captured_at_unix_ms))))
                                .unwrap_or_else(|| "尚未收到完整快照".to_string())
                        }
                        MetricCard {
                            title: "监听器".to_string(),
                            value: runtime.as_ref().map(|item| item.listeners.len().to_string()).unwrap_or_else(|| "0".to_string()),
                            description: "当前运行中的监听器数量".to_string()
                        }
                        MetricCard {
                            title: "活跃连接".to_string(),
                            value: runtime.as_ref().map(|item| item.active_connections.to_string()).unwrap_or_else(|| "0".to_string()),
                            description: "当前活跃连接数".to_string()
                        }
                        MetricCard {
                            title: "路由".to_string(),
                            value: runtime.as_ref().map(|item| item.total_routes.to_string()).unwrap_or_else(|| "0".to_string()),
                            description: "当前路由总数".to_string()
                        }
                        MetricCard {
                            title: "上游".to_string(),
                            value: runtime.as_ref().map(|item| item.total_upstreams.to_string()).unwrap_or_else(|| "0".to_string()),
                            description: "当前上游服务总数".to_string()
                        }
                        MetricCard {
                            title: "DNS 监听".to_string(),
                            value: format_bool(Some(dns_runtime.as_ref().map(|item| item.enabled).unwrap_or(false)), "已启用", "未启用"),
                            description: dns_bind_summary(dns_runtime.as_ref())
                        }
                        MetricCard {
                            title: "DNS 版本".to_string(),
                            value: dns_runtime
                                .as_ref()
                                .and_then(|item| item.published_revision_version.clone())
                                .unwrap_or_else(|| "-".to_string()),
                            description: dns_runtime
                                .as_ref()
                                .and_then(|item| item.published_revision_id.clone())
                                .unwrap_or_else(|| "节点尚未装载权威 DNS 版本".to_string())
                        }
                        MetricCard {
                            title: "DNS 查询".to_string(),
                            value: dns_runtime
                                .as_ref()
                                .map(|item| item.query_total.to_string())
                                .unwrap_or_else(|| "0".to_string()),
                            description: "节点本地权威 DNS 累计查询量".to_string()
                        }
                    }

                    section { class: "panel-grid panel-grid--triptych",
                        SummaryPanel {
                            title: "节点摘要".to_string(),
                            badge: detail.node.running_version.clone(),
                            body: vec![
                                ("节点地址".to_string(), detail.node.advertise_addr.clone()),
                                ("节点角色".to_string(), detail.node.role.clone()),
                                ("管理套接字".to_string(), detail.node.admin_socket_path.clone()),
                                ("状态说明".to_string(), detail.node.status_reason.clone().unwrap_or_else(|| "正常".to_string())),
                                ("最近快照".to_string(), format_optional(detail.node.last_snapshot_version)),
                                ("运行版本号".to_string(), format_optional(detail.node.runtime_revision)),
                                ("运行 PID".to_string(), format_optional(detail.node.runtime_pid)),
                                ("最近在线".to_string(), format_unix_ms(Some(detail.node.last_seen_unix_ms))),
                                (
                                    "已加载模块".to_string(),
                                    snapshot
                                        .as_ref()
                                        .map(|item| format_list(item.included_modules.clone()))
                                        .unwrap_or_else(|| "-".to_string()),
                                ),
                            ]
                        }
                        SummaryPanel {
                            title: "运行摘要".to_string(),
                            badge: runtime
                                .as_ref()
                                .and_then(|item| item.revision)
                                .map(|value| format!("版本 {value}"))
                                .unwrap_or_else(|| "版本 -".to_string()),
                            body: runtime_summary_rows(runtime.as_ref(), counters.as_ref())
                        }
                        SummaryPanel {
                            title: "DNS 摘要".to_string(),
                            badge: dns_runtime
                                .as_ref()
                                .and_then(|item| item.published_revision_version.clone())
                                .unwrap_or_else(|| "DNS -".to_string()),
                            body: dns_runtime_summary_rows(dns_runtime.as_ref())
                        }
                    }

                    section { class: "panel-grid panel-grid--pair",
                        article { class: "panel",
                            header { class: "panel__header",
                                h2 { "DNS 热点请求" }
                                span { "{dns_hot_queries.len()} 条" }
                            }
                            if dns_hot_queries.is_empty() {
                                p { class: "empty-state", "当前节点还没有上报 DNS 热点请求统计。" }
                            } else {
                                div { class: "list-stack",
                                    for query in dns_hot_queries.iter() {
                                        article {
                                            key: "node-hot:{query.qname}:{query.record_type.as_str()}",
                                            class: "list-card",
                                            strong { "{query.qname}" }
                                            div { class: "cell-meta", "{dns_query_zone_label(query)} · {query.record_type.as_str()}" }
                                            div { class: "cell-meta", "查询 {query.query_total} · 返回 {query.answer_total}" }
                                            div { class: "cell-meta", "NOERROR {query.response_noerror_total} · NXDOMAIN {query.response_nxdomain_total} · SERVFAIL {query.response_servfail_total}" }
                                        }
                                    }
                                }
                            }
                        }
                        article { class: "panel",
                            header { class: "panel__header",
                                h2 { "DNS 异常请求" }
                                span { "{dns_error_queries.len()} 条" }
                            }
                            if dns_error_queries.is_empty() {
                                p { class: "empty-state", "当前节点还没有上报 DNS 异常请求统计。" }
                            } else {
                                div { class: "list-stack",
                                    for query in dns_error_queries.iter() {
                                        article {
                                            key: "node-error:{query.qname}:{query.record_type.as_str()}",
                                            class: "list-card",
                                            strong { "{query.qname}" }
                                            div { class: "cell-meta", "{dns_query_zone_label(query)} · {query.record_type.as_str()}" }
                                            div { class: "cell-meta", "异常 {dns_query_error_total(query)} · 查询 {query.query_total}" }
                                            div { class: "cell-meta", "NXDOMAIN {query.response_nxdomain_total} · SERVFAIL {query.response_servfail_total}" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    article { class: "panel",
                        header { class: "panel__header",
                            h2 { "监听器" }
                            span { "{runtime.as_ref().map(|item| item.listeners.len()).unwrap_or(0)} 个" }
                        }
                        if let Some(runtime) = runtime.as_ref() {
                            if runtime.listeners.is_empty() {
                                p { class: "empty-state", "当前快照中没有监听器信息。" }
                            } else {
                                div { class: "table-scroll",
                                    table { class: "data-table",
                                        thead {
                                        tr {
                                                th { "监听器" }
                                                th { "地址" }
                                                th { "特性" }
                                                th { "默认证书" }
                                                th { "绑定数" }
                                            }
                                        }
                                        tbody {
                                            for listener in runtime.listeners.iter() {
                                                tr { key: "{listener.listener_id}",
                                                    td { strong { "{listener.listener_name}" } }
                                                    td { "{listener.listen_addr}" }
                                                    td {
                                                        {format!(
                                                            "TLS {} · HTTP/3 {} · Proxy Protocol {}",
                                                            bool_switch_label(listener.tls_enabled),
                                                            bool_switch_label(listener.http3_enabled),
                                                            bool_switch_label(listener.proxy_protocol_enabled)
                                                        )}
                                                    }
                                                    td { "{listener.default_certificate.clone().unwrap_or_else(|| \"-\".to_string())}" }
                                                    td { "{listener.bindings.len()}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            p { class: "empty-state", "当前没有可解析的监听器运行数据。" }
                        }
                    }

                    section { class: "panel-grid panel-grid--pair",
                        article { class: "panel",
                            header { class: "panel__header",
                                h2 { "最近快照" }
                                span { "{detail.recent_snapshots.len()} 条" }
                            }
                            if detail.recent_snapshots.is_empty() {
                                p { class: "empty-state", "没有历史快照记录。" }
                            } else {
                                div { class: "list-stack",
                                    for snapshot_meta in detail.recent_snapshots.iter() {
                                        article { class: "list-card",
                                            strong { "快照 #{snapshot_meta.snapshot_version}" }
                                            div { class: "cell-meta", "PID {snapshot_meta.pid} · Schema {snapshot_meta.schema_version}" }
                                            div { class: "cell-meta", "{snapshot_meta.binary_version}" }
                                            div { class: "cell-meta", "{format_unix_ms(Some(snapshot_meta.captured_at_unix_ms))}" }
                                        }
                                    }
                                }
                            }
                        }
                        article { class: "panel",
                            header { class: "panel__header",
                                h2 { "最近事件" }
                                span { "{detail.recent_events.len()} 条" }
                            }
                            if detail.recent_events.is_empty() {
                                p { class: "empty-state", "没有最近审计事件。" }
                            } else {
                                div { class: "list-stack",
                                    for event in detail.recent_events.iter() {
                                        article { class: "list-card",
                                            strong { "{event.action}" }
                                            div { class: "cell-meta", "{event.actor_id} · {event.result}" }
                                            div { class: "cell-meta", "{event.resource_type}/{event.resource_id}" }
                                            div { class: "cell-meta", "{format_unix_ms(Some(event.created_at_unix_ms))}" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    details { class: "panel panel--stack panel--diagnostic",
                        summary { class: "panel__header panel__header--summary",
                            span { class: "panel__header-title", "补充诊断数据" }
                            span { "按需展开原始运行 JSON" }
                        }
                        div { class: "code-grid",
                            CodeBlock {
                                title: "status".to_string(),
                                content: snapshot
                                    .as_ref()
                                    .and_then(|item| item.status.as_ref().map(pretty_json))
                                    .unwrap_or_else(|| "{}".to_string())
                            }
                            CodeBlock {
                                title: "traffic".to_string(),
                                content: snapshot
                                    .as_ref()
                                    .and_then(|item| item.traffic.as_ref().map(pretty_json))
                                    .unwrap_or_else(|| "{}".to_string())
                            }
                            CodeBlock {
                                title: "upstreams".to_string(),
                                content: snapshot
                                    .as_ref()
                                    .and_then(|item| item.upstreams.as_ref().map(pretty_json))
                                    .unwrap_or_else(|| "{}".to_string())
                            }
                        }
                    }

                    if let Some(traffic) = traffic.as_ref() {
                        article { class: "panel",
                            header { class: "panel__header",
                                h2 { "流量摘要" }
                                span { "{traffic.listeners.len()} 个监听器 / {traffic.routes.len()} 条路由" }
                            }
                            div { class: "detail-grid",
                                div { strong { "监听器条目" } div { class: "cell-meta", "{traffic.listeners.len()}" } }
                                div { strong { "虚拟主机条目" } div { class: "cell-meta", "{traffic.vhosts.len()}" } }
                                div { strong { "路由条目" } div { class: "cell-meta", "{traffic.routes.len()}" } }
                                div { strong { "上游健康项" } div { class: "cell-meta", "{upstream_health.len()}" } }
                                div { strong { "上游统计项" } div { class: "cell-meta", "{upstream_stats.len()}" } }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn NodeTls(node_id: String) -> Element {
    let session = use_session();
    let actor = (session.actor)();
    let mut detail = use_signal(|| None::<NodeDetailResponse>);
    let mut loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let stream_state = use_signal(StreamState::default);
    let mut reload_serial = use_signal(|| 0_u64);
    let stream = use_signal(|| None::<EventStream>);
    let reload_tick = reload_serial();

    use_drop(move || close_event_stream(stream));

    let actor_snapshot = actor.clone();
    let node_id_for_effect = node_id.clone();
    use_effect(use_reactive!(|(actor_snapshot, node_id_for_effect, reload_tick)| {
        let _ = reload_tick;
        close_event_stream(stream);
        if actor_snapshot.is_none() {
            detail.set(None);
            loading.set(false);
            return;
        }
        to_owned![session, detail, loading, error, stream_state, stream];
        let current_node_id = node_id_for_effect.clone();
        spawn(async move {
            loading.set(true);
            error.set(None);
            match api::get_node_detail(&current_node_id).await {
                Ok(value) => {
                    detail.set(Some(value));
                    stream_state.set(StreamState::Connecting);
                    match api::ensure_events_session().await {
                        Ok(()) => {
                            match build_node_stream(&current_node_id, detail, error, stream_state) {
                                Ok(handle) => stream.set(Some(handle)),
                                Err(stream_error) => {
                                    error.set(Some(stream_error.to_string()));
                                    stream_state.set(StreamState::Error);
                                }
                            }
                        }
                        Err(stream_error) => {
                            if handle_api_auth_error(&stream_error, session) {
                                loading.set(false);
                                return;
                            }
                            error.set(Some(stream_error.to_string()));
                            stream_state.set(StreamState::Error);
                        }
                    }
                }
                Err(load_error) => {
                    if handle_api_auth_error(&load_error, session) {
                        loading.set(false);
                        return;
                    }
                    error.set(Some(load_error.to_string()));
                }
            }
            loading.set(false);
        });
    }));

    let detail_value = detail();
    let snapshot = detail_value.as_ref().and_then(|detail| detail.latest_snapshot.clone());
    let runtime = snapshot.as_ref().and_then(|snapshot| parse_runtime(Some(snapshot)));
    let tls = runtime.as_ref().map(|runtime| runtime.tls.clone()).unwrap_or_default();
    let mtls = runtime.as_ref().map(|runtime| runtime.mtls.clone()).unwrap_or_default();
    let tls_title = detail_value
        .as_ref()
        .map(|item| item.node.node_id.clone())
        .unwrap_or_else(|| node_id.clone());
    let tls_heading = format!("{tls_title} TLS / OCSP");
    let tls_cluster_label = detail_value
        .as_ref()
        .map(|item| item.node.cluster_id.clone())
        .unwrap_or_else(|| "-".to_string());
    let tls_user_label =
        actor.as_ref().map(|item| item.user.username.clone()).unwrap_or_else(|| "-".to_string());
    let tls_snapshot_label = snapshot
        .as_ref()
        .map(|item| item.snapshot_version.to_string())
        .unwrap_or_else(|| "-".to_string());
    let tls_captured_label = snapshot
        .as_ref()
        .map(|item| format_unix_ms(Some(item.captured_at_unix_ms)))
        .unwrap_or_else(|| "-".to_string());
    let tls_identity_name = actor
        .as_ref()
        .map(|item| item.user.display_name.clone())
        .unwrap_or_else(|| "访客".to_string());
    let tls_identity_roles = actor.as_ref().map(role_summary).unwrap_or_else(|| "-".to_string());
    let tls_error_tone = if detail_value.is_some() { "warn" } else { "error" };

    rsx! {
        section { class: "page-shell page-shell--detail",
            header { class: "hero",
                div {
                    p { class: "eyebrow", "节点 TLS" }
                    div { class: "breadcrumb-row",
                        Link { class: "breadcrumb-link", to: Route::Dashboard {}, "总览" }
                        span { "/" }
                        Link { class: "breadcrumb-link", to: Route::NodeDetail { node_id: node_id.clone() }, {tls_title.clone()} }
                        span { "/" }
                        span { "TLS 诊断" }
                    }
                    h1 { {tls_heading.clone()} }
                    p { class: "hero-copy", "查看该节点的证书、OCSP、mTLS、SNI 绑定和上游 TLS 状态，便于排查证书与握手问题。" }
                }
                div { class: "hero-meta",
                    p { strong { "所属集群" } span { {tls_cluster_label.clone()} } }
                    p { strong { "节点状态" } " "
                        if let Some(detail) = detail_value.as_ref() {
                            span { class: format!("state-pill state-pill--{}", detail.node.state.as_str()), "{status_label(detail.node.state.as_str())}" }
                        } else {
                            span { "-" }
                        }
                    }
                    p { strong { "实时流" } " "
                        span { class: format!("realtime-pill realtime-pill--{}", stream_state_label(stream_state())), "{stream_state_text(stream_state())}" }
                    }
                    p { strong { "当前用户" } span { {tls_user_label.clone()} } }
                    p { strong { "快照版本" } span { {tls_snapshot_label.clone()} } }
                    p { strong { "采集时间" } span { {tls_captured_label.clone()} } }
                }
            }

            if actor.is_none() {
                AuthRequired { message: "登录后才能查看节点 TLS 页面。" }
            } else {
                section { class: "toolbar",
                    div { class: "toolbar-links",
                        Link { class: "secondary-button secondary-button--link", to: Route::Dashboard {}, "总览" }
                        Link { class: "secondary-button secondary-button--link", to: Route::NodeDetail { node_id: node_id.clone() }, "节点详情" }
                        button {
                            class: "secondary-button",
                            onclick: move |_| reload_serial += 1,
                            "刷新 TLS 状态"
                        }
                    }
                    div { class: "identity-card",
                        p { class: "identity-card__name", {tls_identity_name.clone()} }
                        p { class: "identity-card__meta", {tls_identity_roles.clone()} }
                    }
                }

                if loading() {
                    StateBanner { tone: "info", message: "正在加载 TLS 详情…" }
                }
                if let Some(message) = error() {
                    StateBanner { tone: tls_error_tone, message }
                }

                if detail_value.is_some() {
                    section { class: "metric-grid",
                        MetricCard { title: "TLS 状态".to_string(), value: format_bool(Some(runtime.as_ref().map(|item| item.tls_enabled).unwrap_or(false)), "已启用", "未启用"), description: "节点当前 TLS 总状态".to_string() }
                        MetricCard { title: "TLS 监听器".to_string(), value: tls.listeners.len().to_string(), description: "带证书或 TLS 配置的监听器数量".to_string() }
                        MetricCard { title: "证书数量".to_string(), value: tls.certificates.len().to_string(), description: "快照中的证书条目数量".to_string() }
                        MetricCard { title: "即将到期".to_string(), value: tls.expiring_certificate_count.to_string(), description: "即将到期的证书数量".to_string() }
                        MetricCard { title: "OCSP 状态".to_string(), value: tls.ocsp.len().to_string(), description: "OCSP 状态条目数量".to_string() }
                        MetricCard { title: "上游 TLS".to_string(), value: runtime.as_ref().map(|item| item.upstream_tls.len().to_string()).unwrap_or_else(|| "0".to_string()), description: "上游 TLS 配置与校验概览".to_string() }
                    }

                    section { class: "panel-grid panel-grid--pair",
                        SummaryPanel {
                            title: "TLS 摘要".to_string(),
                            badge: format!("{} 个监听器", tls.listeners.len()),
                            body: vec![
                                ("能力摘要".to_string(), format!(
                                    "TLS {} · 必选 mTLS {} · 0-RTT 监听器 {}",
                                    bool_switch_label(runtime.as_ref().map(|item| item.tls_enabled).unwrap_or(false)),
                                    mtls.required_listeners,
                                    runtime.as_ref().map(|item| item.http3_early_data_enabled_listeners).unwrap_or(0),
                                )),
                                ("即将到期证书".to_string(), tls.expiring_certificate_count.to_string()),
                                ("SNI 绑定".to_string(), tls.sni_bindings.len().to_string()),
                                ("SNI 冲突".to_string(), tls.sni_conflicts.len().to_string()),
                                ("默认证书绑定".to_string(), tls.default_certificate_bindings.len().to_string()),
                                ("可热更新字段".to_string(), format_list(tls.reload_boundary.reloadable_fields.clone())),
                                ("需重启字段".to_string(), format_list(tls.reload_boundary.restart_required_fields.clone())),
                            ]
                        }
                        SummaryPanel {
                            title: "mTLS 摘要".to_string(),
                            badge: format!("{} 个必选", mtls.required_listeners),
                            body: vec![
                                ("已配置".to_string(), mtls.configured_listeners.to_string()),
                                ("可选认证".to_string(), mtls.optional_listeners.to_string()),
                                ("强制认证".to_string(), mtls.required_listeners.to_string()),
                                ("已认证连接".to_string(), mtls.authenticated_connections.to_string()),
                                ("已认证请求".to_string(), mtls.authenticated_requests.to_string()),
                                ("匿名请求".to_string(), mtls.anonymous_requests.to_string()),
                                ("握手失败".to_string(), mtls.handshake_failures_total.to_string()),
                                ("校验深度超限".to_string(), mtls.handshake_failures_verify_depth_exceeded.to_string()),
                            ]
                        }
                    }

                    TlsListenerPanel { tls: tls.clone() }
                    TlsCertificatesPanel { tls: tls.clone() }
                    TlsOcspPanel { tls: tls.clone() }

                    details { class: "panel panel--stack panel--diagnostic",
                        summary { class: "panel__header panel__header--summary",
                            span { class: "panel__header-title", "补充 TLS 诊断数据" }
                            span { "按需展开原始 JSON" }
                        }
                        div { class: "code-grid",
                            CodeBlock {
                                title: "status.tls".to_string(),
                                content: runtime
                                    .as_ref()
                                    .map(|item| pretty_json(&item.tls))
                                    .unwrap_or_else(|| "{}".to_string())
                            }
                            CodeBlock {
                                title: "status.mtls".to_string(),
                                content: runtime
                                    .as_ref()
                                    .map(|item| pretty_json(&item.mtls))
                                    .unwrap_or_else(|| "{}".to_string())
                            }
                            CodeBlock {
                                title: "status.upstream_tls".to_string(),
                                content: runtime
                                    .as_ref()
                                    .map(|item| pretty_json(&item.upstream_tls))
                                    .unwrap_or_else(|| "[]".to_string())
                            }
                        }
                    }
                }
            }
        }
    }
}
