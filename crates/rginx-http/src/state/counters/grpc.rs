#[derive(Debug, Default)]
struct GrpcTrafficCounters {
    requests_total: AtomicU64,
    protocol_grpc_total: AtomicU64,
    protocol_grpc_web_total: AtomicU64,
    protocol_grpc_web_text_total: AtomicU64,
    status_0_total: AtomicU64,
    status_1_total: AtomicU64,
    status_3_total: AtomicU64,
    status_4_total: AtomicU64,
    status_7_total: AtomicU64,
    status_8_total: AtomicU64,
    status_12_total: AtomicU64,
    status_14_total: AtomicU64,
    status_other_total: AtomicU64,
}

impl GrpcTrafficCounters {
    fn record_request(&self, protocol: &str) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        match protocol {
            "grpc" => {
                self.protocol_grpc_total.fetch_add(1, Ordering::Relaxed);
            }
            "grpc-web" => {
                self.protocol_grpc_web_total.fetch_add(1, Ordering::Relaxed);
            }
            "grpc-web-text" => {
                self.protocol_grpc_web_text_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    fn record_status(&self, status: Option<&str>) {
        match status {
            Some("0") => {
                self.status_0_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("1") => {
                self.status_1_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("3") => {
                self.status_3_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("4") => {
                self.status_4_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("7") => {
                self.status_7_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("8") => {
                self.status_8_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("12") => {
                self.status_12_total.fetch_add(1, Ordering::Relaxed);
            }
            Some("14") => {
                self.status_14_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                self.status_other_total.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn snapshot(&self) -> GrpcTrafficSnapshot {
        GrpcTrafficSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            protocol_grpc_total: self.protocol_grpc_total.load(Ordering::Relaxed),
            protocol_grpc_web_total: self.protocol_grpc_web_total.load(Ordering::Relaxed),
            protocol_grpc_web_text_total: self.protocol_grpc_web_text_total.load(Ordering::Relaxed),
            status_0_total: self.status_0_total.load(Ordering::Relaxed),
            status_1_total: self.status_1_total.load(Ordering::Relaxed),
            status_3_total: self.status_3_total.load(Ordering::Relaxed),
            status_4_total: self.status_4_total.load(Ordering::Relaxed),
            status_7_total: self.status_7_total.load(Ordering::Relaxed),
            status_8_total: self.status_8_total.load(Ordering::Relaxed),
            status_12_total: self.status_12_total.load(Ordering::Relaxed),
            status_14_total: self.status_14_total.load(Ordering::Relaxed),
            status_other_total: self.status_other_total.load(Ordering::Relaxed),
        }
    }
}
