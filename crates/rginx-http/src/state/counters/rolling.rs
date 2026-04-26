#[derive(Debug, Default)]
struct RollingCounter {
    buckets: Mutex<VecDeque<(u64, u64)>>,
}

#[derive(Debug, Default)]
struct RecentTrafficStatsCounters {
    downstream_requests_total: RollingCounter,
    downstream_responses_total: RollingCounter,
    downstream_responses_2xx_total: RollingCounter,
    downstream_responses_4xx_total: RollingCounter,
    downstream_responses_5xx_total: RollingCounter,
    grpc_requests_total: RollingCounter,
}

#[derive(Debug, Default)]
struct RecentUpstreamStatsCounters {
    downstream_requests_total: RollingCounter,
    peer_attempts_total: RollingCounter,
    completed_responses_total: RollingCounter,
    bad_gateway_responses_total: RollingCounter,
    gateway_timeout_responses_total: RollingCounter,
    failovers_total: RollingCounter,
}

impl RollingCounter {
    fn increment_now(&self) {
        self.increment_at(window_now_secs());
    }

    fn increment_at(&self, second: u64) {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        trim_old_buckets(&mut buckets, second, MAX_RECENT_WINDOW_SECS);
        match buckets.back_mut() {
            Some((bucket_second, count)) if *bucket_second == second => {
                *count += 1;
            }
            _ => buckets.push_back((second, 1)),
        }
    }

    fn sum_recent(&self, now_second: u64, window_secs: u64) -> u64 {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        trim_old_buckets(&mut buckets, now_second, MAX_RECENT_WINDOW_SECS);
        let cutoff = now_second.saturating_sub(window_secs.saturating_sub(1));
        buckets.iter().filter_map(|(second, count)| (*second >= cutoff).then_some(*count)).sum()
    }
}

impl RecentTrafficStatsCounters {
    fn snapshot(&self) -> RecentTrafficStatsSnapshot {
        self.snapshot_for_window(RECENT_WINDOW_SECS)
    }

    fn snapshot_for_window(&self, window_secs: u64) -> RecentTrafficStatsSnapshot {
        let now = window_now_secs();
        RecentTrafficStatsSnapshot {
            window_secs,
            downstream_requests_total: self.downstream_requests_total.sum_recent(now, window_secs),
            downstream_responses_total: self
                .downstream_responses_total
                .sum_recent(now, window_secs),
            downstream_responses_2xx_total: self
                .downstream_responses_2xx_total
                .sum_recent(now, window_secs),
            downstream_responses_4xx_total: self
                .downstream_responses_4xx_total
                .sum_recent(now, window_secs),
            downstream_responses_5xx_total: self
                .downstream_responses_5xx_total
                .sum_recent(now, window_secs),
            grpc_requests_total: self.grpc_requests_total.sum_recent(now, window_secs),
        }
    }

    fn record_downstream_request(&self) {
        self.downstream_requests_total.increment_now();
    }

    fn record_downstream_response(&self, status: StatusCode) {
        self.downstream_responses_total.increment_now();
        match status.as_u16() / 100 {
            2 => self.downstream_responses_2xx_total.increment_now(),
            4 => self.downstream_responses_4xx_total.increment_now(),
            5 => self.downstream_responses_5xx_total.increment_now(),
            _ => {}
        }
    }

    fn record_grpc_request(&self) {
        self.grpc_requests_total.increment_now();
    }
}

impl RecentUpstreamStatsCounters {
    fn snapshot(&self) -> RecentUpstreamStatsSnapshot {
        self.snapshot_for_window(RECENT_WINDOW_SECS)
    }

    fn snapshot_for_window(&self, window_secs: u64) -> RecentUpstreamStatsSnapshot {
        let now = window_now_secs();
        RecentUpstreamStatsSnapshot {
            window_secs,
            downstream_requests_total: self.downstream_requests_total.sum_recent(now, window_secs),
            peer_attempts_total: self.peer_attempts_total.sum_recent(now, window_secs),
            completed_responses_total: self.completed_responses_total.sum_recent(now, window_secs),
            bad_gateway_responses_total: self
                .bad_gateway_responses_total
                .sum_recent(now, window_secs),
            gateway_timeout_responses_total: self
                .gateway_timeout_responses_total
                .sum_recent(now, window_secs),
            failovers_total: self.failovers_total.sum_recent(now, window_secs),
        }
    }
}

fn window_now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or(0)
}

fn trim_old_buckets(buckets: &mut VecDeque<(u64, u64)>, now_second: u64, window_secs: u64) {
    let cutoff = now_second.saturating_sub(window_secs.saturating_sub(1));
    while buckets.front().is_some_and(|(second, _)| *second < cutoff) {
        buckets.pop_front();
    }
}
