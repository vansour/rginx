use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, anyhow};
use rginx_runtime::admin::{
    AdminRequest, AdminResponse, RevisionSnapshot, admin_socket_path_for_config,
};

use crate::cli::{Command, DeltaArgs, SnapshotArgs, SnapshotModuleArg, WaitArgs, WindowArgs};

pub(crate) fn run_admin_command(config_path: &Path, command: &Command) -> anyhow::Result<bool> {
    match command {
        Command::Snapshot(args) => {
            print_admin_snapshot(config_path, args)?;
            Ok(true)
        }
        Command::SnapshotVersion => {
            print_admin_snapshot_version(config_path)?;
            Ok(true)
        }
        Command::Delta(args) => {
            print_admin_delta(config_path, args)?;
            Ok(true)
        }
        Command::Wait(args) => {
            print_admin_wait(config_path, args)?;
            Ok(true)
        }
        Command::Status => {
            print_admin_status(config_path)?;
            Ok(true)
        }
        Command::Counters => {
            print_admin_counters(config_path)?;
            Ok(true)
        }
        Command::Traffic(args) => {
            print_admin_traffic(config_path, args)?;
            Ok(true)
        }
        Command::Peers => {
            print_admin_peers(config_path)?;
            Ok(true)
        }
        Command::Upstreams(args) => {
            print_admin_upstreams(config_path, args)?;
            Ok(true)
        }
        Command::Check | Command::MigrateNginx(_) => Ok(false),
    }
}

fn print_admin_status(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetStatus)? {
        AdminResponse::Status(status) => {
            print_record(
                "status",
                [
                    ("revision", status.revision.to_string()),
                    (
                        "config_path",
                        status
                            .config_path
                            .as_deref()
                            .map(Path::display)
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    ),
                    ("listen", status.listen_addr.to_string()),
                    (
                        "worker_threads",
                        status
                            .worker_threads
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "auto".to_string()),
                    ),
                    ("accept_workers", status.accept_workers.to_string()),
                    ("vhosts", status.total_vhosts.to_string()),
                    ("routes", status.total_routes.to_string()),
                    ("upstreams", status.total_upstreams.to_string()),
                    ("tls", if status.tls_enabled { "enabled" } else { "disabled" }.to_string()),
                    ("active_connections", status.active_connections.to_string()),
                    ("reload_attempts", status.reload.attempts_total.to_string()),
                    ("reload_successes", status.reload.successes_total.to_string()),
                    ("reload_failures", status.reload.failures_total.to_string()),
                    ("last_reload", render_last_reload(status.reload.last_result.as_ref())),
                ],
            );
            Ok(())
        }
        response => Err(unexpected_admin_response("status", &response)),
    }
}

fn print_admin_snapshot(config_path: &Path, args: &SnapshotArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetSnapshot {
            include: requested_snapshot_modules(&args.include),
            window_secs: args.window_secs,
        },
    )? {
        AdminResponse::Snapshot(snapshot) => {
            let rendered = serde_json::to_string_pretty(&snapshot)
                .context("failed to encode snapshot JSON")?;
            println!("{rendered}");
            Ok(())
        }
        response => Err(unexpected_admin_response("snapshot", &response)),
    }
}

fn print_admin_snapshot_version(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetSnapshotVersion)? {
        AdminResponse::SnapshotVersion(snapshot) => {
            println!("snapshot_version={}", snapshot.snapshot_version);
            Ok(())
        }
        response => Err(unexpected_admin_response("snapshot-version", &response)),
    }
}

fn print_admin_delta(config_path: &Path, args: &DeltaArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetDelta {
            since_version: args.since_version,
            include: requested_snapshot_modules(&args.include),
            window_secs: args.window_secs,
        },
    )? {
        AdminResponse::Delta(delta) => {
            let rendered =
                serde_json::to_string_pretty(&delta).context("failed to encode delta JSON")?;
            println!("{rendered}");
            Ok(())
        }
        response => Err(unexpected_admin_response("delta", &response)),
    }
}

fn requested_snapshot_modules(
    include: &[SnapshotModuleArg],
) -> Option<Vec<rginx_http::SnapshotModule>> {
    if include.is_empty() {
        return None;
    }

    Some(include.iter().copied().map(snapshot_module).collect())
}

fn snapshot_module(module: SnapshotModuleArg) -> rginx_http::SnapshotModule {
    match module {
        SnapshotModuleArg::Status => rginx_http::SnapshotModule::Status,
        SnapshotModuleArg::Counters => rginx_http::SnapshotModule::Counters,
        SnapshotModuleArg::Traffic => rginx_http::SnapshotModule::Traffic,
        SnapshotModuleArg::PeerHealth => rginx_http::SnapshotModule::PeerHealth,
        SnapshotModuleArg::Upstreams => rginx_http::SnapshotModule::Upstreams,
    }
}

fn print_admin_wait(config_path: &Path, args: &WaitArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::WaitForSnapshotChange {
            since_version: args.since_version,
            timeout_ms: args.timeout_ms,
        },
    )? {
        AdminResponse::SnapshotVersion(snapshot) => {
            println!("snapshot_version={}", snapshot.snapshot_version);
            Ok(())
        }
        response => Err(unexpected_admin_response("wait", &response)),
    }
}

fn print_admin_counters(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetCounters)? {
        AdminResponse::Counters(counters) => {
            print_record(
                "counters",
                [
                    (
                        "downstream_connections_accepted_total",
                        counters.downstream_connections_accepted.to_string(),
                    ),
                    (
                        "downstream_connections_rejected_total",
                        counters.downstream_connections_rejected.to_string(),
                    ),
                    ("downstream_requests_total", counters.downstream_requests.to_string()),
                    ("downstream_responses_total", counters.downstream_responses.to_string()),
                    (
                        "downstream_responses_1xx_total",
                        counters.downstream_responses_1xx.to_string(),
                    ),
                    (
                        "downstream_responses_2xx_total",
                        counters.downstream_responses_2xx.to_string(),
                    ),
                    (
                        "downstream_responses_3xx_total",
                        counters.downstream_responses_3xx.to_string(),
                    ),
                    (
                        "downstream_responses_4xx_total",
                        counters.downstream_responses_4xx.to_string(),
                    ),
                    (
                        "downstream_responses_5xx_total",
                        counters.downstream_responses_5xx.to_string(),
                    ),
                ],
            );
            Ok(())
        }
        response => Err(unexpected_admin_response("counters", &response)),
    }
}

fn print_admin_traffic(config_path: &Path, args: &WindowArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetTrafficStats { window_secs: args.window_secs },
    )? {
        AdminResponse::TrafficStats(traffic) => {
            for listener in traffic.listeners {
                let listener_id = listener.listener_id.clone();
                print_record(
                    "traffic_listener",
                    [
                        ("listener", listener_id.clone()),
                        ("name", listener.listener_name),
                        ("listen", listener.listen_addr.to_string()),
                        ("active_connections", listener.active_connections.to_string()),
                        (
                            "downstream_connections_accepted_total",
                            listener.downstream_connections_accepted.to_string(),
                        ),
                        (
                            "downstream_connections_rejected_total",
                            listener.downstream_connections_rejected.to_string(),
                        ),
                        ("downstream_requests_total", listener.downstream_requests.to_string()),
                        ("unmatched_requests_total", listener.unmatched_requests_total.to_string()),
                        ("downstream_responses_total", listener.downstream_responses.to_string()),
                        (
                            "downstream_responses_1xx_total",
                            listener.downstream_responses_1xx.to_string(),
                        ),
                        (
                            "downstream_responses_2xx_total",
                            listener.downstream_responses_2xx.to_string(),
                        ),
                        (
                            "downstream_responses_3xx_total",
                            listener.downstream_responses_3xx.to_string(),
                        ),
                        (
                            "downstream_responses_4xx_total",
                            listener.downstream_responses_4xx.to_string(),
                        ),
                        (
                            "downstream_responses_5xx_total",
                            listener.downstream_responses_5xx.to_string(),
                        ),
                        ("recent_60s_window_secs", listener.recent_60s.window_secs.to_string()),
                        (
                            "recent_60s_downstream_requests_total",
                            listener.recent_60s.downstream_requests_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_total",
                            listener.recent_60s.downstream_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_2xx_total",
                            listener.recent_60s.downstream_responses_2xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_4xx_total",
                            listener.recent_60s.downstream_responses_4xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_5xx_total",
                            listener.recent_60s.downstream_responses_5xx_total.to_string(),
                        ),
                        (
                            "recent_60s_grpc_requests_total",
                            listener.recent_60s.grpc_requests_total.to_string(),
                        ),
                        ("grpc_requests_total", listener.grpc.requests_total.to_string()),
                        ("grpc_protocol_grpc_total", listener.grpc.protocol_grpc_total.to_string()),
                        (
                            "grpc_protocol_grpc_web_total",
                            listener.grpc.protocol_grpc_web_total.to_string(),
                        ),
                        (
                            "grpc_protocol_grpc_web_text_total",
                            listener.grpc.protocol_grpc_web_text_total.to_string(),
                        ),
                        ("grpc_status_0_total", listener.grpc.status_0_total.to_string()),
                        ("grpc_status_1_total", listener.grpc.status_1_total.to_string()),
                        ("grpc_status_3_total", listener.grpc.status_3_total.to_string()),
                        ("grpc_status_4_total", listener.grpc.status_4_total.to_string()),
                        ("grpc_status_7_total", listener.grpc.status_7_total.to_string()),
                        ("grpc_status_8_total", listener.grpc.status_8_total.to_string()),
                        ("grpc_status_12_total", listener.grpc.status_12_total.to_string()),
                        ("grpc_status_14_total", listener.grpc.status_14_total.to_string()),
                        ("grpc_status_other_total", listener.grpc.status_other_total.to_string()),
                    ],
                );
                if let Some(recent_window) = &listener.recent_window {
                    print_record(
                        "traffic_listener_recent_window",
                        [
                            ("listener", listener_id),
                            ("recent_window_secs", recent_window.window_secs.to_string()),
                            (
                                "recent_window_downstream_requests_total",
                                recent_window.downstream_requests_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_total",
                                recent_window.downstream_responses_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_2xx_total",
                                recent_window.downstream_responses_2xx_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_4xx_total",
                                recent_window.downstream_responses_4xx_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_5xx_total",
                                recent_window.downstream_responses_5xx_total.to_string(),
                            ),
                            (
                                "recent_window_grpc_requests_total",
                                recent_window.grpc_requests_total.to_string(),
                            ),
                        ],
                    );
                }
            }
            for vhost in traffic.vhosts {
                let vhost_id = vhost.vhost_id.clone();
                print_record(
                    "traffic_vhost",
                    [
                        ("vhost", vhost_id.clone()),
                        (
                            "server_names",
                            if vhost.server_names.is_empty() {
                                "-".to_string()
                            } else {
                                vhost.server_names.join(",")
                            },
                        ),
                        ("downstream_requests_total", vhost.downstream_requests.to_string()),
                        ("unmatched_requests_total", vhost.unmatched_requests_total.to_string()),
                        ("downstream_responses_total", vhost.downstream_responses.to_string()),
                        (
                            "downstream_responses_1xx_total",
                            vhost.downstream_responses_1xx.to_string(),
                        ),
                        (
                            "downstream_responses_2xx_total",
                            vhost.downstream_responses_2xx.to_string(),
                        ),
                        (
                            "downstream_responses_3xx_total",
                            vhost.downstream_responses_3xx.to_string(),
                        ),
                        (
                            "downstream_responses_4xx_total",
                            vhost.downstream_responses_4xx.to_string(),
                        ),
                        (
                            "downstream_responses_5xx_total",
                            vhost.downstream_responses_5xx.to_string(),
                        ),
                        ("recent_60s_window_secs", vhost.recent_60s.window_secs.to_string()),
                        (
                            "recent_60s_downstream_requests_total",
                            vhost.recent_60s.downstream_requests_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_total",
                            vhost.recent_60s.downstream_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_2xx_total",
                            vhost.recent_60s.downstream_responses_2xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_4xx_total",
                            vhost.recent_60s.downstream_responses_4xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_5xx_total",
                            vhost.recent_60s.downstream_responses_5xx_total.to_string(),
                        ),
                        (
                            "recent_60s_grpc_requests_total",
                            vhost.recent_60s.grpc_requests_total.to_string(),
                        ),
                        ("grpc_requests_total", vhost.grpc.requests_total.to_string()),
                        ("grpc_protocol_grpc_total", vhost.grpc.protocol_grpc_total.to_string()),
                        (
                            "grpc_protocol_grpc_web_total",
                            vhost.grpc.protocol_grpc_web_total.to_string(),
                        ),
                        (
                            "grpc_protocol_grpc_web_text_total",
                            vhost.grpc.protocol_grpc_web_text_total.to_string(),
                        ),
                        ("grpc_status_0_total", vhost.grpc.status_0_total.to_string()),
                        ("grpc_status_1_total", vhost.grpc.status_1_total.to_string()),
                        ("grpc_status_3_total", vhost.grpc.status_3_total.to_string()),
                        ("grpc_status_4_total", vhost.grpc.status_4_total.to_string()),
                        ("grpc_status_7_total", vhost.grpc.status_7_total.to_string()),
                        ("grpc_status_8_total", vhost.grpc.status_8_total.to_string()),
                        ("grpc_status_12_total", vhost.grpc.status_12_total.to_string()),
                        ("grpc_status_14_total", vhost.grpc.status_14_total.to_string()),
                        ("grpc_status_other_total", vhost.grpc.status_other_total.to_string()),
                    ],
                );
                if let Some(recent_window) = &vhost.recent_window {
                    print_record(
                        "traffic_vhost_recent_window",
                        [
                            ("vhost", vhost_id),
                            ("recent_window_secs", recent_window.window_secs.to_string()),
                            (
                                "recent_window_downstream_requests_total",
                                recent_window.downstream_requests_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_total",
                                recent_window.downstream_responses_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_2xx_total",
                                recent_window.downstream_responses_2xx_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_4xx_total",
                                recent_window.downstream_responses_4xx_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_5xx_total",
                                recent_window.downstream_responses_5xx_total.to_string(),
                            ),
                            (
                                "recent_window_grpc_requests_total",
                                recent_window.grpc_requests_total.to_string(),
                            ),
                        ],
                    );
                }
            }
            for route in traffic.routes {
                let route_id = route.route_id.clone();
                print_record(
                    "traffic_route",
                    [
                        ("route", route_id.clone()),
                        ("vhost", route.vhost_id.clone()),
                        ("downstream_requests_total", route.downstream_requests.to_string()),
                        ("downstream_responses_total", route.downstream_responses.to_string()),
                        (
                            "downstream_responses_1xx_total",
                            route.downstream_responses_1xx.to_string(),
                        ),
                        (
                            "downstream_responses_2xx_total",
                            route.downstream_responses_2xx.to_string(),
                        ),
                        (
                            "downstream_responses_3xx_total",
                            route.downstream_responses_3xx.to_string(),
                        ),
                        (
                            "downstream_responses_4xx_total",
                            route.downstream_responses_4xx.to_string(),
                        ),
                        (
                            "downstream_responses_5xx_total",
                            route.downstream_responses_5xx.to_string(),
                        ),
                        ("access_denied_total", route.access_denied_total.to_string()),
                        ("rate_limited_total", route.rate_limited_total.to_string()),
                        ("recent_60s_window_secs", route.recent_60s.window_secs.to_string()),
                        (
                            "recent_60s_downstream_requests_total",
                            route.recent_60s.downstream_requests_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_total",
                            route.recent_60s.downstream_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_2xx_total",
                            route.recent_60s.downstream_responses_2xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_4xx_total",
                            route.recent_60s.downstream_responses_4xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_5xx_total",
                            route.recent_60s.downstream_responses_5xx_total.to_string(),
                        ),
                        (
                            "recent_60s_grpc_requests_total",
                            route.recent_60s.grpc_requests_total.to_string(),
                        ),
                        ("grpc_requests_total", route.grpc.requests_total.to_string()),
                        ("grpc_protocol_grpc_total", route.grpc.protocol_grpc_total.to_string()),
                        (
                            "grpc_protocol_grpc_web_total",
                            route.grpc.protocol_grpc_web_total.to_string(),
                        ),
                        (
                            "grpc_protocol_grpc_web_text_total",
                            route.grpc.protocol_grpc_web_text_total.to_string(),
                        ),
                        ("grpc_status_0_total", route.grpc.status_0_total.to_string()),
                        ("grpc_status_1_total", route.grpc.status_1_total.to_string()),
                        ("grpc_status_3_total", route.grpc.status_3_total.to_string()),
                        ("grpc_status_4_total", route.grpc.status_4_total.to_string()),
                        ("grpc_status_7_total", route.grpc.status_7_total.to_string()),
                        ("grpc_status_8_total", route.grpc.status_8_total.to_string()),
                        ("grpc_status_12_total", route.grpc.status_12_total.to_string()),
                        ("grpc_status_14_total", route.grpc.status_14_total.to_string()),
                        ("grpc_status_other_total", route.grpc.status_other_total.to_string()),
                    ],
                );
                if let Some(recent_window) = &route.recent_window {
                    print_record(
                        "traffic_route_recent_window",
                        [
                            ("route", route_id),
                            ("recent_window_secs", recent_window.window_secs.to_string()),
                            (
                                "recent_window_downstream_requests_total",
                                recent_window.downstream_requests_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_total",
                                recent_window.downstream_responses_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_2xx_total",
                                recent_window.downstream_responses_2xx_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_4xx_total",
                                recent_window.downstream_responses_4xx_total.to_string(),
                            ),
                            (
                                "recent_window_downstream_responses_5xx_total",
                                recent_window.downstream_responses_5xx_total.to_string(),
                            ),
                            (
                                "recent_window_grpc_requests_total",
                                recent_window.grpc_requests_total.to_string(),
                            ),
                        ],
                    );
                }
            }
            Ok(())
        }
        response => Err(unexpected_admin_response("traffic", &response)),
    }
}

fn print_admin_peers(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetPeerHealth)? {
        AdminResponse::PeerHealth(upstreams) => {
            for upstream in upstreams {
                let upstream_name = upstream.upstream_name.clone();
                print_record(
                    "peer_health_upstream",
                    [
                        ("upstream", upstream_name.clone()),
                        ("unhealthy_after_failures", upstream.unhealthy_after_failures.to_string()),
                        ("cooldown_ms", upstream.cooldown_ms.to_string()),
                        ("active_health_enabled", upstream.active_health_enabled.to_string()),
                    ],
                );
                for peer in upstream.peers {
                    print_record(
                        "peer_health_peer",
                        [
                            ("upstream", upstream_name.clone()),
                            ("peer", peer.peer_url),
                            ("backup", peer.backup.to_string()),
                            ("weight", peer.weight.to_string()),
                            ("available", peer.available.to_string()),
                            ("passive_failures", peer.passive_consecutive_failures.to_string()),
                            (
                                "passive_cooldown_remaining_ms",
                                peer.passive_cooldown_remaining_ms
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            ("passive_pending_recovery", peer.passive_pending_recovery.to_string()),
                            ("active_unhealthy", peer.active_unhealthy.to_string()),
                            ("active_successes", peer.active_consecutive_successes.to_string()),
                            ("active_requests", peer.active_requests.to_string()),
                        ],
                    );
                }
            }
            Ok(())
        }
        response => Err(unexpected_admin_response("peers", &response)),
    }
}

fn print_admin_upstreams(config_path: &Path, args: &WindowArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetUpstreamStats { window_secs: args.window_secs },
    )? {
        AdminResponse::UpstreamStats(upstreams) => {
            for upstream in upstreams {
                let upstream_name = upstream.upstream_name.clone();
                print_record(
                    "upstream_stats",
                    [
                        ("upstream", upstream_name.clone()),
                        (
                            "downstream_requests_total",
                            upstream.downstream_requests_total.to_string(),
                        ),
                        ("peer_attempts_total", upstream.peer_attempts_total.to_string()),
                        ("peer_successes_total", upstream.peer_successes_total.to_string()),
                        ("peer_failures_total", upstream.peer_failures_total.to_string()),
                        ("peer_timeouts_total", upstream.peer_timeouts_total.to_string()),
                        ("failovers_total", upstream.failovers_total.to_string()),
                        (
                            "completed_responses_total",
                            upstream.completed_responses_total.to_string(),
                        ),
                        (
                            "bad_gateway_responses_total",
                            upstream.bad_gateway_responses_total.to_string(),
                        ),
                        (
                            "gateway_timeout_responses_total",
                            upstream.gateway_timeout_responses_total.to_string(),
                        ),
                        (
                            "bad_request_responses_total",
                            upstream.bad_request_responses_total.to_string(),
                        ),
                        (
                            "payload_too_large_responses_total",
                            upstream.payload_too_large_responses_total.to_string(),
                        ),
                        (
                            "unsupported_media_type_responses_total",
                            upstream.unsupported_media_type_responses_total.to_string(),
                        ),
                        ("no_healthy_peers_total", upstream.no_healthy_peers_total.to_string()),
                        ("recent_60s_window_secs", upstream.recent_60s.window_secs.to_string()),
                        (
                            "recent_60s_downstream_requests_total",
                            upstream.recent_60s.downstream_requests_total.to_string(),
                        ),
                        (
                            "recent_60s_peer_attempts_total",
                            upstream.recent_60s.peer_attempts_total.to_string(),
                        ),
                        (
                            "recent_60s_completed_responses_total",
                            upstream.recent_60s.completed_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_bad_gateway_responses_total",
                            upstream.recent_60s.bad_gateway_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_gateway_timeout_responses_total",
                            upstream.recent_60s.gateway_timeout_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_failovers_total",
                            upstream.recent_60s.failovers_total.to_string(),
                        ),
                    ],
                );
                if let Some(recent_window) = &upstream.recent_window {
                    print_record(
                        "upstream_stats_recent_window",
                        [
                            ("upstream", upstream_name.clone()),
                            ("recent_window_secs", recent_window.window_secs.to_string()),
                            (
                                "recent_window_downstream_requests_total",
                                recent_window.downstream_requests_total.to_string(),
                            ),
                            (
                                "recent_window_peer_attempts_total",
                                recent_window.peer_attempts_total.to_string(),
                            ),
                            (
                                "recent_window_completed_responses_total",
                                recent_window.completed_responses_total.to_string(),
                            ),
                            (
                                "recent_window_bad_gateway_responses_total",
                                recent_window.bad_gateway_responses_total.to_string(),
                            ),
                            (
                                "recent_window_gateway_timeout_responses_total",
                                recent_window.gateway_timeout_responses_total.to_string(),
                            ),
                            (
                                "recent_window_failovers_total",
                                recent_window.failovers_total.to_string(),
                            ),
                        ],
                    );
                }
                for peer in upstream.peers {
                    print_record(
                        "upstream_stats_peer",
                        [
                            ("upstream", upstream_name.clone()),
                            ("peer", peer.peer_url),
                            ("attempts_total", peer.attempts_total.to_string()),
                            ("successes_total", peer.successes_total.to_string()),
                            ("failures_total", peer.failures_total.to_string()),
                            ("timeouts_total", peer.timeouts_total.to_string()),
                        ],
                    );
                }
            }
            Ok(())
        }
        response => Err(unexpected_admin_response("upstreams", &response)),
    }
}

fn query_admin_socket(config_path: &Path, request: AdminRequest) -> anyhow::Result<AdminResponse> {
    let socket_path = admin_socket_path_for_config(config_path);
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to admin socket {}", socket_path.display()))?;
    serde_json::to_writer(&mut stream, &request)
        .context("failed to encode admin socket request")?;
    stream.write_all(b"\n").context("failed to terminate admin socket request")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to shutdown admin socket write side")?;

    let mut response = String::new();
    BufReader::new(stream)
        .read_to_string(&mut response)
        .context("failed to read admin socket response")?;
    let response: AdminResponse =
        serde_json::from_str(response.trim()).context("failed to decode admin socket response")?;
    match response {
        AdminResponse::Error { message } => Err(anyhow!("admin socket error: {message}")),
        response => Ok(response),
    }
}

fn unexpected_admin_response(command: &str, response: &AdminResponse) -> anyhow::Error {
    anyhow!("unexpected admin response for `{command}`: {}", admin_response_kind(response))
}

fn admin_response_kind(response: &AdminResponse) -> &'static str {
    match response {
        AdminResponse::Snapshot(_) => "snapshot",
        AdminResponse::SnapshotVersion(_) => "snapshot_version",
        AdminResponse::Delta(_) => "delta",
        AdminResponse::Status(_) => "status",
        AdminResponse::Counters(_) => "counters",
        AdminResponse::TrafficStats(_) => "traffic_stats",
        AdminResponse::PeerHealth(_) => "peer_health",
        AdminResponse::UpstreamStats(_) => "upstream_stats",
        AdminResponse::Revision(RevisionSnapshot { .. }) => "revision",
        AdminResponse::Error { .. } => "error",
    }
}

fn render_last_reload(result: Option<&rginx_http::ReloadResultSnapshot>) -> String {
    let Some(result) = result else {
        return "-".to_string();
    };

    let finished_at = result
        .finished_at_unix_ms
        .checked_div(1000)
        .and_then(|seconds| UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds)))
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|| result.finished_at_unix_ms.to_string());

    match &result.outcome {
        rginx_http::ReloadOutcomeSnapshot::Success { revision } => {
            format!("success revision={revision} finished_at_unix_s={finished_at}")
        }
        rginx_http::ReloadOutcomeSnapshot::Failure { error } => {
            format!("failure error={error:?} finished_at_unix_s={finished_at}")
        }
    }
}

fn print_record<const N: usize>(kind: &str, fields: [(&str, String); N]) {
    let mut rendered = String::from("kind=");
    rendered.push_str(kind);
    for (key, value) in fields {
        rendered.push(' ');
        rendered.push_str(key);
        rendered.push('=');
        rendered.push_str(&encode_record_value(&value));
    }
    println!("{rendered}");
}

fn encode_record_value(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '.' | ':' | '/' | '-' | '_' | ',' | '*' | '[' | ']' | '|')
        })
    {
        value.to_string()
    } else {
        serde_json::to_string(value).expect("record value should encode as JSON string")
    }
}
