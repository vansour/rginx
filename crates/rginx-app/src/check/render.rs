mod listeners;
mod tls;

use std::path::Path;

use super::summary::CheckSummary;

pub(crate) fn print_check_success(config_path: &Path, summary: CheckSummary) {
    print_configuration_summary(config_path, &summary);
    listeners::print_listener_details(&summary);
    print_route_transport_details(&summary);
    tls::print_tls_details(&summary);
}

fn print_configuration_summary(config_path: &Path, summary: &CheckSummary) {
    println!(
        "configuration is valid: config={} listener_model={} listeners={} listener_bindings={} listen_addrs={} bind_addrs={} tls={} http3={} http3_early_data_enabled_listeners={} vhosts={} routes={} upstreams={} cache_zones={} cache_enabled_routes={} worker_threads={} accept_workers={}",
        config_path.display(),
        summary.listener_model,
        summary.listener_count,
        summary.listener_binding_count,
        render_listener_addrs(summary),
        render_binding_addrs(summary),
        render_enabled(summary.tls_enabled),
        render_enabled(summary.http3_enabled),
        summary.http3_early_data_enabled_listeners,
        summary.total_vhost_count,
        summary.total_route_count,
        summary.upstream_count,
        summary.cache_zone_count,
        summary.cache_enabled_route_count,
        summary
            .worker_threads
            .map(|count: usize| count.to_string())
            .unwrap_or_else(|| "auto".to_string()),
        summary.accept_workers,
    );
}

fn print_route_transport_details(summary: &CheckSummary) {
    println!(
        "route_transport_details=request_buffering_auto={} request_buffering_on={} request_buffering_off={} response_buffering_auto={} response_buffering_on={} response_buffering_off={} compression_auto={} compression_off={} compression_force={} custom_compression_min_bytes_routes={} custom_compression_content_types_routes={} streaming_response_idle_timeout_routes={} cache_enabled_routes={}",
        summary.route_transport.request_buffering_auto_routes,
        summary.route_transport.request_buffering_on_routes,
        summary.route_transport.request_buffering_off_routes,
        summary.route_transport.response_buffering_auto_routes,
        summary.route_transport.response_buffering_on_routes,
        summary.route_transport.response_buffering_off_routes,
        summary.route_transport.compression_auto_routes,
        summary.route_transport.compression_off_routes,
        summary.route_transport.compression_force_routes,
        summary.route_transport.custom_compression_min_bytes_routes,
        summary.route_transport.custom_compression_content_types_routes,
        summary.route_transport.streaming_response_idle_timeout_routes,
        summary.route_transport.cache_enabled_routes,
    );
    for zone in &summary.cache_zones {
        println!(
            "check_cache_zone name={} path={} max_size_bytes={} inactive_secs={} default_ttl_secs={} max_entry_bytes={}",
            zone.name,
            zone.path.display(),
            zone.max_size_bytes.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
            zone.inactive_secs,
            zone.default_ttl_secs,
            zone.max_entry_bytes,
        );
    }
}

fn render_listener_addrs(summary: &CheckSummary) -> String {
    if summary.listeners.is_empty() {
        return "-".to_string();
    }

    summary
        .listeners
        .iter()
        .map(|listener| listener.listen_addr.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn render_binding_addrs(summary: &CheckSummary) -> String {
    if summary.listeners.is_empty() {
        return "-".to_string();
    }

    let bindings = summary
        .listeners
        .iter()
        .flat_map(|listener| {
            listener
                .bindings
                .iter()
                .map(|binding| format!("{}://{}", binding.transport, binding.listen_addr))
        })
        .collect::<Vec<_>>();

    if bindings.is_empty() { "-".to_string() } else { bindings.join(",") }
}

fn render_enabled(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

fn render_string_list(values: &[String]) -> String {
    if values.is_empty() { "-".to_string() } else { values.join(",") }
}
