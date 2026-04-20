use super::*;

fn zero_trend_series() -> Vec<f64> {
    vec![0.0; 30]
}

fn default_trend_x_ticks() -> Vec<String> {
    vec![
        "30 天前".to_string(),
        "23 天前".to_string(),
        "16 天前".to_string(),
        "9 天前".to_string(),
        "今天".to_string(),
    ]
}

fn traffic_y_ticks() -> Vec<String> {
    vec![
        "0".to_string(),
        "250 GB".to_string(),
        "500 GB".to_string(),
        "750 GB".to_string(),
        "1 TB".to_string(),
    ]
}

fn bandwidth_y_ticks() -> Vec<String> {
    vec![
        "0".to_string(),
        "250 Mbps".to_string(),
        "500 Mbps".to_string(),
        "750 Mbps".to_string(),
        "1 Gbps".to_string(),
    ]
}

#[component]
pub fn Dashboard() -> Element {
    let session = use_session();
    let actor = (session.actor)();
    let session_ready = (session.ready)();
    let mut dashboard = use_signal(|| None::<DashboardSummary>);
    let mut accelerated_sites = use_signal(|| None::<u32>);
    let mut loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let mut stream_state = use_signal(StreamState::default);
    let stream = use_signal(|| None::<EventStream>);

    use_drop(move || close_event_stream(stream));

    let actor_snapshot = actor.clone();
    use_effect(use_reactive!(|(actor_snapshot,)| {
        close_event_stream(stream);
        if actor_snapshot.is_none() {
            dashboard.set(None);
            accelerated_sites.set(None);
            stream_state.set(StreamState::Idle);
            loading.set(false);
            return;
        }

        to_owned![session, dashboard, accelerated_sites, loading, error, stream_state, stream];
        spawn(async move {
            loading.set(true);
            error.set(None);
            match api::get_dashboard().await {
                Ok(dashboard_value) => {
                    accelerated_sites.set(None);
                    dashboard.set(Some(dashboard_value));
                    stream_state.set(StreamState::Connecting);

                    match api::ensure_events_session().await {
                        Ok(()) => match build_dashboard_stream(dashboard, error, stream_state) {
                            Ok(handle) => stream.set(Some(handle)),
                            Err(stream_error) => {
                                error.set(Some(stream_error.to_string()));
                                stream_state.set(StreamState::Error);
                            }
                        },
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

    let dashboard_snapshot = dashboard();
    let latest_revision_id = dashboard_snapshot.as_ref().and_then(|item| {
        item.latest_revision.as_ref().map(|revision| revision.revision_id.clone())
    });
    let actor_snapshot = actor.clone();
    use_effect(use_reactive!(|(actor_snapshot, latest_revision_id)| {
        if actor_snapshot.is_none() {
            accelerated_sites.set(None);
            return;
        }

        let Some(revision_id) = latest_revision_id.clone() else {
            accelerated_sites.set(Some(0));
            return;
        };

        to_owned![session, accelerated_sites, error];
        spawn(async move {
            match api::get_revision(&revision_id).await {
                Ok(detail) => {
                    let count = detail
                        .compile_summary
                        .map(|summary| summary.total_vhost_count)
                        .unwrap_or(0);
                    accelerated_sites.set(Some(count));
                }
                Err(load_error) => {
                    if handle_api_auth_error(&load_error, session) {
                        return;
                    }
                    accelerated_sites.set(None);
                    error.set(Some(load_error.to_string()));
                }
            }
        });
    }));

    let dashboard_error_tone = if actor.is_some() { "warn" } else { "error" };
    let trend_x_ticks = default_trend_x_ticks();
    let traffic_trend = zero_trend_series();
    let bandwidth_trend = zero_trend_series();
    let overview_cards = vec![
        (
            "加速网站数量",
            accelerated_sites().map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
            "按最新已发布版本中的站点数统计".to_string(),
        ),
        ("本月总流量", "--".to_string(), "待接入月流量统计".to_string()),
        ("本月带宽峰值", "--".to_string(), "待接入月峰值带宽".to_string()),
        ("今日流量", "--".to_string(), "待接入日流量统计".to_string()),
        ("今日带宽峰值", "--".to_string(), "待接入日峰值带宽".to_string()),
    ];

    rsx! {
        section { class: "page-shell page-shell--overview",
            if !session_ready {
                StateBanner { tone: "info", message: "正在同步本地会话…" }
            } else if let Some(message) = error() {
                StateBanner { tone: dashboard_error_tone, message }
            }

            if actor.is_none() {
                AuthRequired { message: "登录后才能查看控制台总览。" }
            } else if dashboard_snapshot.is_some() {
                if loading() {
                    StateBanner { tone: "info", message: "正在刷新总览数据…" }
                }

                section { class: "overview-summary-grid",
                    for (title, value, description) in overview_cards {
                        article { class: "overview-summary-card",
                            p { class: "overview-summary-card__label", "{title}" }
                            p { class: "overview-summary-card__value", "{value}" }
                            p { class: "overview-summary-card__meta", "{description}" }
                        }
                    }
                }

                section { class: "overview-trend-grid",
                    OverviewTrendChart {
                        title: "近 30 日流量趋势".to_string(),
                        subtitle: "按天查看总流量变化，数据接入后会自动替换当前占位状态。".to_string(),
                        series: traffic_trend.clone(),
                        y_ticks: traffic_y_ticks(),
                        x_ticks: trend_x_ticks.clone(),
                        empty_message: Some("暂无近 30 日流量历史数据".to_string()),
                    }
                    OverviewTrendChart {
                        title: "近 30 日带宽趋势".to_string(),
                        subtitle: "按天查看带宽峰值变化，图表结构已就位，等待后端时间序列接入。".to_string(),
                        series: bandwidth_trend,
                        y_ticks: bandwidth_y_ticks(),
                        x_ticks: trend_x_ticks,
                        empty_message: Some("暂无近 30 日带宽历史数据".to_string()),
                    }
                }
            }
        }
    }
}
