use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Metrics {
    inner: Arc<MetricsInner>,
}

#[derive(Default)]
struct MetricsInner {
    active_connections: AtomicU64,
    http_requests_total: Mutex<BTreeMap<HttpRequestKey, u64>>,
    http_rate_limited_total: Mutex<BTreeMap<RouteKey, u64>>,
    http_request_duration_ms: Mutex<BTreeMap<RouteKey, Histogram>>,
    upstream_requests_total: Mutex<BTreeMap<UpstreamRequestKey, u64>>,
    active_health_checks_total: Mutex<BTreeMap<ActiveHealthCheckKey, u64>>,
    config_reloads_total: Mutex<BTreeMap<ConfigReloadKey, u64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HttpRequestKey {
    route: String,
    status: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RouteKey {
    route: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct UpstreamRequestKey {
    upstream: String,
    peer: String,
    result: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ActiveHealthCheckKey {
    upstream: String,
    peer: String,
    result: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ConfigReloadKey {
    result: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Histogram {
    bucket_counts: Vec<u64>,
    count: u64,
    sum: u64,
}

const HTTP_REQUEST_DURATION_BUCKETS_MS: [u64; 11] =
    [5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000];

impl Histogram {
    fn new() -> Self {
        Self { bucket_counts: vec![0; HTTP_REQUEST_DURATION_BUCKETS_MS.len()], count: 0, sum: 0 }
    }

    fn observe(&mut self, value: u64) {
        self.count += 1;
        self.sum += value;

        for (index, bucket) in HTTP_REQUEST_DURATION_BUCKETS_MS.iter().enumerate() {
            if value <= *bucket {
                self.bucket_counts[index] += 1;
                break;
            }
        }
    }
}

impl Metrics {
    pub fn increment_active_connections(&self) {
        self.inner.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_active_connections(&self) {
        self.inner.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn active_connections(&self) -> u64 {
        self.inner.active_connections.load(Ordering::Relaxed)
    }

    pub fn observe_http_request(&self, route: &str, status: u16, elapsed_ms: u64) {
        increment_counter(
            &self.inner.http_requests_total,
            HttpRequestKey { route: route.to_string(), status },
        );

        let mut histograms = lock_map(&self.inner.http_request_duration_ms);
        histograms
            .entry(RouteKey { route: route.to_string() })
            .or_insert_with(Histogram::new)
            .observe(elapsed_ms);
    }

    pub fn record_rate_limited(&self, route: &str) {
        increment_counter(
            &self.inner.http_rate_limited_total,
            RouteKey { route: route.to_string() },
        );
    }

    pub fn record_upstream_request(&self, upstream: &str, peer: &str, result: &str) {
        increment_counter(
            &self.inner.upstream_requests_total,
            UpstreamRequestKey {
                upstream: upstream.to_string(),
                peer: peer.to_string(),
                result: result.to_string(),
            },
        );
    }

    pub fn record_active_health_check(&self, upstream: &str, peer: &str, result: &str) {
        increment_counter(
            &self.inner.active_health_checks_total,
            ActiveHealthCheckKey {
                upstream: upstream.to_string(),
                peer: peer.to_string(),
                result: result.to_string(),
            },
        );
    }

    pub fn record_config_reload(&self, result: &str) {
        increment_counter(
            &self.inner.config_reloads_total,
            ConfigReloadKey { result: result.to_string() },
        );
    }

    pub fn render_prometheus(&self) -> String {
        let mut output = String::new();

        output.push_str("# HELP rginx_active_connections Current active client connections.\n");
        output.push_str("# TYPE rginx_active_connections gauge\n");
        output.push_str(&format!(
            "rginx_active_connections {}\n",
            self.inner.active_connections.load(Ordering::Relaxed)
        ));

        render_counter_family(
            &mut output,
            "rginx_http_requests_total",
            "Total HTTP requests handled by route and status.",
            &*lock_map(&self.inner.http_requests_total),
            |key, value, out| {
                out.push_str(&format!(
                    "rginx_http_requests_total{{route=\"{}\",status=\"{}\"}} {}\n",
                    escape_label_value(&key.route),
                    key.status,
                    value
                ));
            },
        );

        render_counter_family(
            &mut output,
            "rginx_http_rate_limited_total",
            "Total HTTP requests rejected by route rate limiting.",
            &*lock_map(&self.inner.http_rate_limited_total),
            |key, value, out| {
                out.push_str(&format!(
                    "rginx_http_rate_limited_total{{route=\"{}\"}} {}\n",
                    escape_label_value(&key.route),
                    value
                ));
            },
        );

        output.push_str(
            "# HELP rginx_http_request_duration_ms HTTP request duration histogram in milliseconds.\n",
        );
        output.push_str("# TYPE rginx_http_request_duration_ms histogram\n");
        for (key, histogram) in lock_map(&self.inner.http_request_duration_ms).iter() {
            let mut cumulative = 0u64;
            for (index, bucket) in HTTP_REQUEST_DURATION_BUCKETS_MS.iter().enumerate() {
                cumulative += histogram.bucket_counts[index];
                output.push_str(&format!(
                    "rginx_http_request_duration_ms_bucket{{route=\"{}\",le=\"{}\"}} {}\n",
                    escape_label_value(&key.route),
                    bucket,
                    cumulative
                ));
            }
            output.push_str(&format!(
                "rginx_http_request_duration_ms_bucket{{route=\"{}\",le=\"+Inf\"}} {}\n",
                escape_label_value(&key.route),
                histogram.count
            ));
            output.push_str(&format!(
                "rginx_http_request_duration_ms_sum{{route=\"{}\"}} {}\n",
                escape_label_value(&key.route),
                histogram.sum
            ));
            output.push_str(&format!(
                "rginx_http_request_duration_ms_count{{route=\"{}\"}} {}\n",
                escape_label_value(&key.route),
                histogram.count
            ));
        }

        render_counter_family(
            &mut output,
            "rginx_upstream_requests_total",
            "Total upstream request attempts by peer and result.",
            &*lock_map(&self.inner.upstream_requests_total),
            |key, value, out| {
                out.push_str(&format!(
                    "rginx_upstream_requests_total{{upstream=\"{}\",peer=\"{}\",result=\"{}\"}} {}\n",
                    escape_label_value(&key.upstream),
                    escape_label_value(&key.peer),
                    escape_label_value(&key.result),
                    value
                ));
            },
        );

        render_counter_family(
            &mut output,
            "rginx_active_health_checks_total",
            "Total active health check probes by peer and result.",
            &*lock_map(&self.inner.active_health_checks_total),
            |key, value, out| {
                out.push_str(&format!(
                    "rginx_active_health_checks_total{{upstream=\"{}\",peer=\"{}\",result=\"{}\"}} {}\n",
                    escape_label_value(&key.upstream),
                    escape_label_value(&key.peer),
                    escape_label_value(&key.result),
                    value
                ));
            },
        );

        render_counter_family(
            &mut output,
            "rginx_config_reloads_total",
            "Total configuration reload attempts by result.",
            &*lock_map(&self.inner.config_reloads_total),
            |key, value, out| {
                out.push_str(&format!(
                    "rginx_config_reloads_total{{result=\"{}\"}} {}\n",
                    escape_label_value(&key.result),
                    value
                ));
            },
        );

        output
    }
}

fn increment_counter<K>(map: &Mutex<BTreeMap<K, u64>>, key: K)
where
    K: Ord,
{
    let mut map = lock_map(map);
    *map.entry(key).or_insert(0) += 1;
}

fn render_counter_family<K>(
    output: &mut String,
    metric_name: &str,
    help: &str,
    entries: &BTreeMap<K, u64>,
    mut render_entry: impl FnMut(&K, u64, &mut String),
) {
    output.push_str(&format!("# HELP {metric_name} {help}\n"));
    output.push_str(&format!("# TYPE {metric_name} counter\n"));
    for (key, value) in entries {
        render_entry(key, *value, output);
    }
}

fn escape_label_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn lock_map<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[test]
    fn render_prometheus_includes_all_metric_families() {
        let metrics = Metrics::default();
        metrics.increment_active_connections();
        metrics.observe_http_request("exact:/status", 200, 12);
        metrics.record_rate_limited("prefix:/api");
        metrics.record_upstream_request("backend", "http://127.0.0.1:9000", "success");
        metrics.record_active_health_check("backend", "http://127.0.0.1:9000", "healthy");
        metrics.record_config_reload("success");

        let rendered = metrics.render_prometheus();

        assert!(rendered.contains("rginx_active_connections 1"));
        assert!(rendered
            .contains("rginx_http_requests_total{route=\"exact:/status\",status=\"200\"} 1"));
        assert!(rendered.contains("rginx_http_rate_limited_total{route=\"prefix:/api\"} 1"));
        assert!(
            rendered.contains("rginx_http_request_duration_ms_count{route=\"exact:/status\"} 1")
        );
        assert!(rendered.contains(
            "rginx_upstream_requests_total{upstream=\"backend\",peer=\"http://127.0.0.1:9000\",result=\"success\"} 1"
        ));
        assert!(rendered.contains(
            "rginx_active_health_checks_total{upstream=\"backend\",peer=\"http://127.0.0.1:9000\",result=\"healthy\"} 1"
        ));
        assert!(rendered.contains("rginx_config_reloads_total{result=\"success\"} 1"));
    }

    #[test]
    fn render_prometheus_uses_cumulative_histogram_buckets_once() {
        let metrics = Metrics::default();
        metrics.observe_http_request("exact:/status", 200, 12);

        let rendered = metrics.render_prometheus();

        assert!(rendered
            .contains("rginx_http_request_duration_ms_bucket{route=\"exact:/status\",le=\"5\"} 0"));
        assert!(rendered.contains(
            "rginx_http_request_duration_ms_bucket{route=\"exact:/status\",le=\"10\"} 0"
        ));
        assert!(rendered.contains(
            "rginx_http_request_duration_ms_bucket{route=\"exact:/status\",le=\"25\"} 1"
        ));
        assert!(rendered.contains(
            "rginx_http_request_duration_ms_bucket{route=\"exact:/status\",le=\"50\"} 1"
        ));
        assert!(rendered.contains(
            "rginx_http_request_duration_ms_bucket{route=\"exact:/status\",le=\"+Inf\"} 1"
        ));
    }
}
