use super::*;

#[component]
pub(super) fn MetricCard(title: String, value: String, description: String) -> Element {
    rsx! {
        article { class: "metric-card",
            p { class: "metric-card__title", {title} }
            p { class: "metric-card__value", {value} }
            p { class: "metric-card__description", {description} }
        }
    }
}

#[component]
pub(super) fn StateBanner(tone: &'static str, message: String) -> Element {
    rsx! {
        p {
            class: format!("state-banner state-banner--{tone}"),
            {message}
        }
    }
}

#[component]
pub(super) fn AuthRequired(message: &'static str) -> Element {
    rsx! {
        article { class: "panel auth-panel",
            header { class: "panel__header",
                h2 { "需要登录" }
                span { "访问受限" }
            }
            p { {message} }
            Link { class: "primary-button", to: Route::Login {}, "前往登录" }
        }
    }
}

#[component]
pub(super) fn SummaryPanel(title: String, badge: String, body: Vec<(String, String)>) -> Element {
    rsx! {
        article { class: "panel",
            header { class: "panel__header",
                h2 { {title} }
                span { {badge} }
            }
            dl { class: "kv-grid",
                for (label, value) in body {
                    div {
                        dt { {label} }
                        dd { {value} }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn CodeBlock(title: String, content: String) -> Element {
    rsx! {
        article { class: "code-panel",
            header { class: "panel__header",
                h2 { {title} }
                span { "JSON / 文本" }
            }
            pre { class: "code-block", {content} }
        }
    }
}

#[component]
pub(super) fn OverviewTrendChart(
    title: String,
    subtitle: String,
    series: Vec<f64>,
    y_ticks: Vec<String>,
    x_ticks: Vec<String>,
    empty_message: Option<String>,
) -> Element {
    let width = 720.0_f64;
    let height = 260.0_f64;
    let left = 52.0_f64;
    let right = 18.0_f64;
    let top = 18.0_f64;
    let bottom = 34.0_f64;
    let chart_width = width - left - right;
    let chart_height = height - top - bottom;
    let max_value = series.iter().copied().fold(0.0_f64, |acc, value| acc.max(value)).max(1.0);
    let has_series = series.iter().any(|value| *value > 0.0);

    let points = series
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = if series.len() <= 1 {
                left
            } else {
                left + (chart_width * index as f64 / (series.len() - 1) as f64)
            };
            let normalized = (value / max_value).clamp(0.0, 1.0);
            let y = top + chart_height - chart_height * normalized;
            format!("{x:.2},{y:.2}")
        })
        .collect::<Vec<_>>()
        .join(" ");

    let area_points = if points.is_empty() {
        String::new()
    } else {
        format!(
            "{left:.2},{:.2} {points} {:.2},{:.2}",
            top + chart_height,
            left + chart_width,
            top + chart_height
        )
    };

    let grid_lines =
        (0..=4).map(|index| top + chart_height * index as f64 / 4.0).collect::<Vec<_>>();

    let x_tick_positions = if x_ticks.len() <= 1 {
        vec![left]
    } else {
        x_ticks
            .iter()
            .enumerate()
            .map(|(index, _)| left + chart_width * index as f64 / (x_ticks.len() - 1) as f64)
            .collect::<Vec<_>>()
    };
    let y_tick_positions = if y_ticks.len() <= 1 {
        vec![top + chart_height]
    } else {
        y_ticks
            .iter()
            .enumerate()
            .map(|(index, _)| {
                top + chart_height * index as f64 / (y_ticks.len().saturating_sub(1).max(1)) as f64
            })
            .collect::<Vec<_>>()
    };
    let x_axis_ticks =
        x_ticks.iter().cloned().zip(x_tick_positions.iter().copied()).collect::<Vec<_>>();
    let y_axis_ticks =
        y_ticks.iter().cloned().zip(y_tick_positions.iter().copied()).collect::<Vec<_>>();

    rsx! {
        article { class: "overview-trend-panel",
            header { class: "overview-trend-panel__header",
                div {
                    h2 { "{title}" }
                    p { "{subtitle}" }
                }
                span { class: "overview-trend-panel__badge", "近 30 日" }
            }

            div { class: "overview-trend-chart",
                svg {
                    class: "overview-trend-chart__svg",
                    view_box: "0 0 720 260",
                    preserve_aspect_ratio: "none",

                    defs {
                        linearGradient { id: "overview-trend-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                            stop { offset: "0%", stop_color: "rgba(47, 128, 237, 0.22)" }
                            stop { offset: "100%", stop_color: "rgba(47, 128, 237, 0.02)" }
                        }
                    }

                    for (index, y) in grid_lines.iter().enumerate() {
                        line {
                            key: "grid:{index}",
                            x1: "{left}",
                            y1: "{y}",
                            x2: "{left + chart_width}",
                            y2: "{y}",
                            class: "overview-trend-chart__grid-line"
                        }
                    }

                    for (index, (label, y)) in y_axis_ticks.iter().enumerate() {
                        text {
                            key: "y:{index}",
                            x: "{left - 10.0}",
                            y: "{y + 4.0}",
                            text_anchor: "end",
                            class: "overview-trend-chart__axis-text",
                            "{label}"
                        }
                    }

                    if !area_points.is_empty() {
                        polygon {
                            points: "{area_points}",
                            class: "overview-trend-chart__area"
                        }
                    }

                    if !points.is_empty() {
                        polyline {
                            points: "{points}",
                            class: "overview-trend-chart__line"
                        }
                    }

                    for (index, (label, x)) in x_axis_ticks.iter().enumerate() {
                        text {
                            key: "x:{index}",
                            x: "{x}",
                            y: "{height - 8.0}",
                            text_anchor: if index == 0 { "start" } else if index + 1 == x_axis_ticks.len() { "end" } else { "middle" },
                            class: "overview-trend-chart__axis-text",
                            "{label}"
                        }
                    }
                }

                if !has_series {
                    if let Some(message) = empty_message {
                    div { class: "overview-trend-chart__empty", "{message}" }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn TlsListenerPanel(tls: TlsRuntimeSnapshot) -> Element {
    rsx! {
        article { class: "panel panel--stack",
            header { class: "panel__header",
                h2 { "TLS 监听器" }
                span { "{tls.listeners.len()} 个" }
            }
            if tls.listeners.is_empty() {
                p { class: "empty-state", "当前快照中没有 TLS 监听器信息。" }
            } else {
                div { class: "table-scroll",
                    table { class: "data-table",
                        thead {
                            tr {
                                th { "监听器" }
                                th { "协议" }
                                th { "HTTP/3" }
                                th { "客户端认证" }
                                th { "证书" }
                            }
                        }
                        tbody {
                            for listener in tls.listeners.iter() {
                                tr { key: "{listener.listener_id}",
                                    td {
                                        strong { {listener.listener_name.clone()} }
                                        div { class: "cell-meta", {listener.listen_addr.clone()} }
                                    }
                                    td { {format_list(listener.alpn_protocols.clone())} }
                                    td {
                                        {format_bool(Some(listener.http3_enabled), "已启用", "未启用")}
                                        div { class: "cell-meta", {listener.http3_listen_addr.clone().unwrap_or_else(|| "-".to_string())} }
                                    }
                                    td {
                                        {listener.client_auth_mode.clone().unwrap_or_else(|| "未启用".to_string())}
                                        div { class: "cell-meta", {format!(
                                            "深度 {} · CRL {}",
                                            format_optional(listener.client_auth_verify_depth),
                                            format_bool(Some(listener.client_auth_crl_configured), "已启用", "未启用")
                                        )} }
                                    }
                                    td {
                                        {listener.default_certificate.clone().unwrap_or_else(|| "-".to_string())}
                                        div { class: "cell-meta", {format!("SNI {}", format_list(listener.sni_names.clone()))} }
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
pub(super) fn TlsCertificatesPanel(tls: TlsRuntimeSnapshot) -> Element {
    rsx! {
        article { class: "panel",
            header { class: "panel__header",
                h2 { "证书" }
                span { "{tls.certificates.len()} 张" }
            }
            if tls.certificates.is_empty() {
                p { class: "empty-state", "没有证书快照数据。" }
            } else {
                div { class: "table-scroll",
                    table { class: "data-table",
                        thead {
                            tr {
                                th { "作用域" }
                                th { "主题 / 颁发者" }
                                th { "到期时间" }
                                th { "默认证书监听器" }
                                th { "OCSP" }
                            }
                        }
                        tbody {
                            for certificate in tls.certificates.iter() {
                                tr { key: "{certificate.scope}",
                                    td {
                                        strong { {certificate.scope.clone()} }
                                        div { class: "cell-meta", {certificate.cert_path.clone()} }
                                    }
                                    td {
                                        {certificate.subject.clone().unwrap_or_else(|| "-".to_string())}
                                        div { class: "cell-meta", {certificate.issuer.clone().unwrap_or_else(|| "-".to_string())} }
                                    }
                                    td {
                                        {format_unix_ms(certificate.not_after_unix_ms)}
                                        div { class: "cell-meta", {format!("{} 天", format_optional(certificate.expires_in_days))} }
                                    }
                                    td { {format_list(certificate.selected_as_default_for_listeners.clone())} }
                                    td {
                                        {format_bool(Some(certificate.ocsp_staple_configured), "已配置", "未启用")}
                                        div { class: "cell-meta", {format_list(certificate.server_names.clone())} }
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
pub(super) fn TlsOcspPanel(tls: TlsRuntimeSnapshot) -> Element {
    rsx! {
        article { class: "panel",
            header { class: "panel__header",
                h2 { "OCSP" }
                span { "{tls.ocsp.len()} 条" }
            }
            if tls.ocsp.is_empty() {
                p { class: "empty-state", "没有 OCSP 快照数据。" }
            } else {
                div { class: "table-scroll",
                    table { class: "data-table",
                        thead {
                            tr {
                                th { "作用域" }
                                th { "响应器" }
                                th { "缓存" }
                                th { "最近刷新" }
                                th { "失败次数" }
                            }
                        }
                        tbody {
                            for ocsp in tls.ocsp.iter() {
                                tr { key: "{ocsp.scope}",
                                    td {
                                        strong { {ocsp.scope.clone()} }
                                        div { class: "cell-meta", {ocsp.cert_path.clone()} }
                                    }
                                    td { {format_list(ocsp.responder_urls.clone())} }
                                    td {
                                        {format_bool(Some(ocsp.cache_loaded), "已加载", "空缓存")}
                                        div { class: "cell-meta", {format!(
                                            "自动刷新 {}",
                                            format_bool(Some(ocsp.auto_refresh_enabled), "已启用", "未启用")
                                        )} }
                                    }
                                    td { {format_unix_ms(ocsp.last_refresh_unix_ms)} }
                                    td {
                                        {ocsp.failures_total.to_string()}
                                        div { class: "cell-meta", {ocsp.last_error.clone().unwrap_or_else(|| "正常".to_string())} }
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
