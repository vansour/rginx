use super::render::print_record;
use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

pub(super) fn print_admin_cache(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetCacheStats)? {
        AdminResponse::CacheStats(cache) => {
            print_cache_stats(&cache, "cache_summary", "cache_zone");
            Ok(())
        }
        response => Err(unexpected_admin_response("cache", &response)),
    }
}

pub(super) fn print_admin_purge_cache(
    config_path: &Path,
    args: &PurgeCacheArgs,
) -> anyhow::Result<()> {
    let request = match (&args.key, &args.prefix) {
        (Some(key), None) => {
            AdminRequest::PurgeCacheKey { zone_name: args.zone.clone(), key: key.clone() }
        }
        (None, Some(prefix)) => {
            AdminRequest::PurgeCachePrefix { zone_name: args.zone.clone(), prefix: prefix.clone() }
        }
        (None, None) => AdminRequest::PurgeCacheZone { zone_name: args.zone.clone() },
        (Some(_), Some(_)) => unreachable!("clap enforces purge cache selector exclusivity"),
    };

    match query_admin_socket(config_path, request)? {
        AdminResponse::CachePurge(result) => {
            print_record(
                "cache_purge",
                [
                    ("zone", result.zone_name),
                    ("scope", result.scope),
                    ("removed_entries", result.removed_entries.to_string()),
                    ("removed_bytes", result.removed_bytes.to_string()),
                ],
            );
            Ok(())
        }
        response => Err(unexpected_admin_response("purge-cache", &response)),
    }
}

pub(super) fn print_status_cache(cache: &rginx_http::CacheStatsSnapshot) {
    print_cache_stats(cache, "status_cache", "status_cache_zone");
}

fn print_cache_stats(cache: &rginx_http::CacheStatsSnapshot, summary_kind: &str, zone_kind: &str) {
    let zones = &cache.zones;
    print_record(
        summary_kind,
        [
            ("zones", zones.len().to_string()),
            ("entries", zones.iter().map(|zone| zone.entry_count).sum::<usize>().to_string()),
            (
                "current_size_bytes",
                zones.iter().map(|zone| zone.current_size_bytes).sum::<usize>().to_string(),
            ),
            ("hit_total", zones.iter().map(|zone| zone.hit_total).sum::<u64>().to_string()),
            ("miss_total", zones.iter().map(|zone| zone.miss_total).sum::<u64>().to_string()),
            ("bypass_total", zones.iter().map(|zone| zone.bypass_total).sum::<u64>().to_string()),
            ("expired_total", zones.iter().map(|zone| zone.expired_total).sum::<u64>().to_string()),
            ("stale_total", zones.iter().map(|zone| zone.stale_total).sum::<u64>().to_string()),
            (
                "updating_total",
                zones.iter().map(|zone| zone.updating_total).sum::<u64>().to_string(),
            ),
            (
                "revalidated_total",
                zones.iter().map(|zone| zone.revalidated_total).sum::<u64>().to_string(),
            ),
            (
                "write_success_total",
                zones.iter().map(|zone| zone.write_success_total).sum::<u64>().to_string(),
            ),
            (
                "write_error_total",
                zones.iter().map(|zone| zone.write_error_total).sum::<u64>().to_string(),
            ),
            (
                "eviction_total",
                zones.iter().map(|zone| zone.eviction_total).sum::<u64>().to_string(),
            ),
            ("purge_total", zones.iter().map(|zone| zone.purge_total).sum::<u64>().to_string()),
            (
                "inactive_cleanup_total",
                zones.iter().map(|zone| zone.inactive_cleanup_total).sum::<u64>().to_string(),
            ),
        ],
    );

    for zone in zones {
        print_record(
            zone_kind,
            [
                ("zone", zone.zone_name.clone()),
                ("path", zone.path.display().to_string()),
                (
                    "max_size_bytes",
                    zone.max_size_bytes
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("inactive_secs", zone.inactive_secs.to_string()),
                ("default_ttl_secs", zone.default_ttl_secs.to_string()),
                ("max_entry_bytes", zone.max_entry_bytes.to_string()),
                ("entry_count", zone.entry_count.to_string()),
                ("current_size_bytes", zone.current_size_bytes.to_string()),
                ("hit_total", zone.hit_total.to_string()),
                ("miss_total", zone.miss_total.to_string()),
                ("bypass_total", zone.bypass_total.to_string()),
                ("expired_total", zone.expired_total.to_string()),
                ("stale_total", zone.stale_total.to_string()),
                ("updating_total", zone.updating_total.to_string()),
                ("revalidated_total", zone.revalidated_total.to_string()),
                ("write_success_total", zone.write_success_total.to_string()),
                ("write_error_total", zone.write_error_total.to_string()),
                ("eviction_total", zone.eviction_total.to_string()),
                ("purge_total", zone.purge_total.to_string()),
                ("inactive_cleanup_total", zone.inactive_cleanup_total.to_string()),
            ],
        );
    }
}
