use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA, CNAME, TXT};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use hickory_proto::serialize::binary::{BinEncodable, BinEncoder};
use ipnet::IpNet;
use rginx_control_types::{
    DnsPlan, DnsPublishedSnapshot, DnsRecordType, DnsRuntimeQueryStat, DnsRuntimeStatus,
    DnsTargetKind, NodeLifecycleState, NodeSummary,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;

#[derive(Debug, Clone)]
pub struct DnsServerConfig {
    pub udp_bind_addr: Option<SocketAddr>,
    pub tcp_bind_addr: Option<SocketAddr>,
}

#[derive(Debug, Clone)]
pub struct DnsAnswer {
    pub record_type: DnsRecordType,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct DnsQueryResult {
    pub response_code: ResponseCode,
    pub ttl_secs: u32,
    pub answers: Vec<DnsAnswer>,
}

impl DnsQueryResult {
    pub fn noerror(ttl_secs: u32, answers: Vec<DnsAnswer>) -> Self {
        Self { response_code: ResponseCode::NoError, ttl_secs, answers }
    }

    pub fn nxdomain() -> Self {
        Self { response_code: ResponseCode::NXDomain, ttl_secs: 0, answers: Vec::new() }
    }

    pub fn servfail() -> Self {
        Self { response_code: ResponseCode::ServFail, ttl_secs: 0, answers: Vec::new() }
    }
}

pub trait DnsQueryEngine: Send + Sync + 'static {
    fn resolve(&self, qname: &str, record_type: DnsRecordType, source_ip: IpAddr)
    -> DnsQueryResult;
}

const DNS_RUNTIME_QUERY_STAT_LIMIT: usize = 256;
const DNS_RUNTIME_HOT_QUERY_LIMIT: usize = 8;
const DNS_RUNTIME_ERROR_QUERY_LIMIT: usize = 8;

#[derive(Debug)]
pub struct InMemoryDnsRuntime {
    udp_bind_addr: Option<SocketAddr>,
    tcp_bind_addr: Option<SocketAddr>,
    snapshots: RwLock<Vec<CompiledDnsSnapshot>>,
    query_total: AtomicU64,
    response_noerror_total: AtomicU64,
    response_nxdomain_total: AtomicU64,
    response_servfail_total: AtomicU64,
    query_stats: RwLock<HashMap<QueryStatKey, QueryStatValue>>,
}

#[derive(Debug, Clone)]
struct CompiledDnsSnapshot {
    cluster_id: String,
    revision_id: String,
    version_label: String,
    plan: DnsPlan,
    nodes_by_id: HashMap<String, NodeSummary>,
    cluster_nodes: Vec<NodeSummary>,
    resolved_upstreams: HashMap<String, Vec<IpAddr>>,
}

#[derive(Debug, Clone)]
struct WeightedAnswer {
    answer: DnsAnswer,
    weight: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct QueryStatKey {
    cluster_id: String,
    zone_name: Option<String>,
    qname: String,
    record_type: DnsRecordType,
}

#[derive(Debug, Clone, Default)]
struct QueryStatValue {
    query_total: u64,
    answer_total: u64,
    response_noerror_total: u64,
    response_nxdomain_total: u64,
    response_servfail_total: u64,
    last_query_at_unix_ms: u64,
}

#[derive(Debug, Clone, Default)]
struct ClusterQueryStats {
    query_total: u64,
    response_noerror_total: u64,
    response_nxdomain_total: u64,
    response_servfail_total: u64,
    hot_queries: Vec<DnsRuntimeQueryStat>,
    error_queries: Vec<DnsRuntimeQueryStat>,
}

impl QueryStatValue {
    fn error_total(&self) -> u64 {
        self.response_nxdomain_total.saturating_add(self.response_servfail_total)
    }

    fn retention_score(&self) -> u128 {
        u128::from(self.query_total)
            .saturating_add(u128::from(self.answer_total))
            .saturating_add(u128::from(self.error_total()).saturating_mul(10))
    }
}

impl InMemoryDnsRuntime {
    pub fn new(udp_bind_addr: Option<SocketAddr>, tcp_bind_addr: Option<SocketAddr>) -> Self {
        Self {
            udp_bind_addr,
            tcp_bind_addr,
            snapshots: RwLock::new(Vec::new()),
            query_total: AtomicU64::new(0),
            response_noerror_total: AtomicU64::new(0),
            response_nxdomain_total: AtomicU64::new(0),
            response_servfail_total: AtomicU64::new(0),
            query_stats: RwLock::new(HashMap::new()),
        }
    }

    pub fn enabled(&self) -> bool {
        self.udp_bind_addr.is_some() || self.tcp_bind_addr.is_some()
    }

    pub fn server_config(&self) -> DnsServerConfig {
        DnsServerConfig { udp_bind_addr: self.udp_bind_addr, tcp_bind_addr: self.tcp_bind_addr }
    }

    pub fn replace_snapshots(&self, snapshots: Vec<DnsPublishedSnapshot>) {
        let compiled = snapshots.into_iter().map(CompiledDnsSnapshot::from_snapshot).collect();
        *self.snapshots.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = compiled;
    }

    pub fn runtime_status(&self) -> Vec<DnsRuntimeStatus> {
        let snapshots = self.snapshots.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshots
            .iter()
            .map(|snapshot| self.build_runtime_status(&snapshot.cluster_id, Some(snapshot)))
            .collect()
    }

    pub fn cluster_status(&self, cluster_id: &str) -> DnsRuntimeStatus {
        let snapshots = self.snapshots.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = snapshots.iter().find(|snapshot| snapshot.cluster_id == cluster_id);
        self.build_runtime_status(cluster_id, snapshot)
    }

    fn build_runtime_status(
        &self,
        cluster_id: &str,
        snapshot: Option<&CompiledDnsSnapshot>,
    ) -> DnsRuntimeStatus {
        let cluster_stats = self.build_cluster_query_stats(cluster_id);
        let query_total = if snapshot.is_some() {
            cluster_stats.query_total
        } else {
            self.query_total.load(Ordering::Relaxed)
        };
        let response_noerror_total = if snapshot.is_some() {
            cluster_stats.response_noerror_total
        } else {
            self.response_noerror_total.load(Ordering::Relaxed)
        };
        let response_nxdomain_total = if snapshot.is_some() {
            cluster_stats.response_nxdomain_total
        } else {
            self.response_nxdomain_total.load(Ordering::Relaxed)
        };
        let response_servfail_total = if snapshot.is_some() {
            cluster_stats.response_servfail_total
        } else {
            self.response_servfail_total.load(Ordering::Relaxed)
        };

        DnsRuntimeStatus {
            enabled: self.enabled(),
            cluster_id: cluster_id.to_string(),
            udp_bind_addr: self.udp_bind_addr.map(|addr| addr.to_string()),
            tcp_bind_addr: self.tcp_bind_addr.map(|addr| addr.to_string()),
            published_revision_id: snapshot.map(|snapshot| snapshot.revision_id.clone()),
            published_revision_version: snapshot.map(|snapshot| snapshot.version_label.clone()),
            zone_count: snapshot
                .map(|snapshot| u32::try_from(snapshot.plan.zones.len()).unwrap_or(u32::MAX))
                .unwrap_or(0),
            record_count: snapshot
                .map(|snapshot| {
                    u32::try_from(
                        snapshot.plan.zones.iter().map(|zone| zone.records.len()).sum::<usize>(),
                    )
                    .unwrap_or(u32::MAX)
                })
                .unwrap_or(0),
            query_total,
            response_noerror_total,
            response_nxdomain_total,
            response_servfail_total,
            hot_queries: cluster_stats.hot_queries,
            error_queries: cluster_stats.error_queries,
        }
    }

    fn build_cluster_query_stats(&self, cluster_id: &str) -> ClusterQueryStats {
        let stats = self.query_stats.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut rows = stats
            .iter()
            .filter(|(key, _)| key.cluster_id == cluster_id)
            .map(|(key, value)| DnsRuntimeQueryStat {
                zone_name: key.zone_name.clone(),
                qname: key.qname.clone(),
                record_type: key.record_type,
                query_total: value.query_total,
                answer_total: value.answer_total,
                response_noerror_total: value.response_noerror_total,
                response_nxdomain_total: value.response_nxdomain_total,
                response_servfail_total: value.response_servfail_total,
                last_query_at_unix_ms: value.last_query_at_unix_ms,
            })
            .collect::<Vec<_>>();

        let query_total = rows.iter().map(|item| item.query_total).sum();
        let response_noerror_total = rows.iter().map(|item| item.response_noerror_total).sum();
        let response_nxdomain_total = rows.iter().map(|item| item.response_nxdomain_total).sum();
        let response_servfail_total = rows.iter().map(|item| item.response_servfail_total).sum();

        rows.sort_by(|left, right| {
            right
                .query_total
                .cmp(&left.query_total)
                .then(right.answer_total.cmp(&left.answer_total))
                .then(right.last_query_at_unix_ms.cmp(&left.last_query_at_unix_ms))
                .then(left.qname.cmp(&right.qname))
                .then(left.record_type.as_str().cmp(right.record_type.as_str()))
        });
        let hot_queries =
            rows.iter().take(DNS_RUNTIME_HOT_QUERY_LIMIT).cloned().collect::<Vec<_>>();

        let mut error_queries = rows
            .into_iter()
            .filter(|item| item.response_nxdomain_total > 0 || item.response_servfail_total > 0)
            .collect::<Vec<_>>();
        error_queries.sort_by(|left, right| {
            right
                .response_nxdomain_total
                .saturating_add(right.response_servfail_total)
                .cmp(&left.response_nxdomain_total.saturating_add(left.response_servfail_total))
                .then(right.response_servfail_total.cmp(&left.response_servfail_total))
                .then(right.query_total.cmp(&left.query_total))
                .then(right.last_query_at_unix_ms.cmp(&left.last_query_at_unix_ms))
                .then(left.qname.cmp(&right.qname))
                .then(left.record_type.as_str().cmp(right.record_type.as_str()))
        });
        error_queries.truncate(DNS_RUNTIME_ERROR_QUERY_LIMIT);

        ClusterQueryStats {
            query_total,
            response_noerror_total,
            response_nxdomain_total,
            response_servfail_total,
            hot_queries,
            error_queries,
        }
    }

    fn record_query_stat(
        &self,
        cluster_id: &str,
        zone_name: Option<&str>,
        qname: &str,
        record_type: DnsRecordType,
        response_code: ResponseCode,
        answer_count: usize,
    ) {
        let mut stats = self.query_stats.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = stats
            .entry(QueryStatKey {
                cluster_id: cluster_id.to_string(),
                zone_name: zone_name.map(ToOwned::to_owned),
                qname: qname.to_string(),
                record_type,
            })
            .or_default();
        entry.query_total = entry.query_total.saturating_add(1);
        entry.answer_total =
            entry.answer_total.saturating_add(u64::try_from(answer_count).unwrap_or(u64::MAX));
        match response_code {
            ResponseCode::NoError => {
                entry.response_noerror_total = entry.response_noerror_total.saturating_add(1);
            }
            ResponseCode::NXDomain => {
                entry.response_nxdomain_total = entry.response_nxdomain_total.saturating_add(1);
            }
            ResponseCode::ServFail => {
                entry.response_servfail_total = entry.response_servfail_total.saturating_add(1);
            }
            _ => {}
        }
        entry.last_query_at_unix_ms = unix_time_ms(SystemTime::now());

        if stats.len() > DNS_RUNTIME_QUERY_STAT_LIMIT {
            prune_query_stats(&mut stats);
        }
    }
}

impl DnsQueryEngine for InMemoryDnsRuntime {
    fn resolve(
        &self,
        qname: &str,
        record_type: DnsRecordType,
        source_ip: IpAddr,
    ) -> DnsQueryResult {
        self.query_total.fetch_add(1, Ordering::Relaxed);
        let snapshots = self.snapshots.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let qname = normalize_name(qname);

        let Some((snapshot, zone_index)) =
            snapshots
                .iter()
                .enumerate()
                .flat_map(|(snapshot_index, snapshot)| {
                    let qname = qname.clone();
                    snapshot.plan.zones.iter().enumerate().filter_map(move |(zone_index, zone)| {
                        let zone_name = normalize_name(&zone.zone_name);
                        (qname == zone_name || qname.ends_with(&format!(".{zone_name}")))
                            .then_some((snapshot_index, zone_index, zone_name.len()))
                    })
                })
                .max_by_key(|(_, _, len)| *len)
                .and_then(|(snapshot_index, zone_index, _)| {
                    snapshots.get(snapshot_index).map(|snapshot| (snapshot, zone_index))
                })
        else {
            self.response_nxdomain_total.fetch_add(1, Ordering::Relaxed);
            if snapshots.len() == 1
                && let Some(snapshot) = snapshots.first()
            {
                self.record_query_stat(
                    &snapshot.cluster_id,
                    None,
                    &qname,
                    record_type,
                    ResponseCode::NXDomain,
                    0,
                );
            }
            return DnsQueryResult::nxdomain();
        };

        let zone = &snapshot.plan.zones[zone_index];
        let zone_name = normalize_name(&zone.zone_name);
        let Some(record) = zone.records.iter().find(|record| {
            normalize_name(&record_fqdn(zone, &record.name)) == qname
                && record.record_type == record_type
        }) else {
            self.response_nxdomain_total.fetch_add(1, Ordering::Relaxed);
            self.record_query_stat(
                &snapshot.cluster_id,
                Some(&zone_name),
                &qname,
                record_type,
                ResponseCode::NXDomain,
                0,
            );
            return DnsQueryResult::nxdomain();
        };

        let mut answers = Vec::new();
        match record.record_type {
            DnsRecordType::A | DnsRecordType::Aaaa => {
                for value in &record.values {
                    if let Ok(ip) = value.parse::<IpAddr>()
                        && ip_matches_record_type(ip, record.record_type)
                    {
                        answers.push(WeightedAnswer {
                            answer: DnsAnswer {
                                record_type: record.record_type,
                                value: ip.to_string(),
                            },
                            weight: 1,
                        });
                    }
                }

                for target in &record.targets {
                    if !target.enabled {
                        continue;
                    }
                    if !target.source_cidrs.is_empty()
                        && !target
                            .source_cidrs
                            .iter()
                            .filter_map(|cidr| cidr.parse::<IpNet>().ok())
                            .any(|cidr| cidr.contains(&source_ip))
                    {
                        continue;
                    }

                    let weight = target.weight.max(1);
                    match target.kind {
                        DnsTargetKind::StaticIp => {
                            if let Ok(ip) = target.value.parse::<IpAddr>()
                                && ip_matches_record_type(ip, record.record_type)
                            {
                                answers.push(WeightedAnswer {
                                    answer: DnsAnswer {
                                        record_type: record.record_type,
                                        value: ip.to_string(),
                                    },
                                    weight,
                                });
                            }
                        }
                        DnsTargetKind::Node => {
                            if let Some(node) = snapshot.nodes_by_id.get(&target.value)
                                && matches!(
                                    node.state,
                                    NodeLifecycleState::Online | NodeLifecycleState::Draining
                                )
                                && let Some(ip) = advertise_ip(&node.advertise_addr)
                                && ip_matches_record_type(ip, record.record_type)
                            {
                                answers.push(WeightedAnswer {
                                    answer: DnsAnswer {
                                        record_type: record.record_type,
                                        value: ip.to_string(),
                                    },
                                    weight,
                                });
                            }
                        }
                        DnsTargetKind::Cluster => {
                            for node in snapshot
                                .cluster_nodes
                                .iter()
                                .filter(|node| node.cluster_id == target.value)
                            {
                                if !matches!(
                                    node.state,
                                    NodeLifecycleState::Online | NodeLifecycleState::Draining
                                ) {
                                    continue;
                                }
                                if let Some(ip) = advertise_ip(&node.advertise_addr)
                                    && ip_matches_record_type(ip, record.record_type)
                                {
                                    answers.push(WeightedAnswer {
                                        answer: DnsAnswer {
                                            record_type: record.record_type,
                                            value: ip.to_string(),
                                        },
                                        weight,
                                    });
                                }
                            }
                        }
                        DnsTargetKind::Upstream => {
                            if let Some(addrs) = snapshot.resolved_upstreams.get(&target.value) {
                                for ip in addrs {
                                    if ip_matches_record_type(*ip, record.record_type) {
                                        answers.push(WeightedAnswer {
                                            answer: DnsAnswer {
                                                record_type: record.record_type,
                                                value: ip.to_string(),
                                            },
                                            weight,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            DnsRecordType::Cname | DnsRecordType::Txt => {
                answers.extend(record.values.iter().cloned().map(|value| WeightedAnswer {
                    answer: DnsAnswer { record_type: record.record_type, value },
                    weight: 1,
                }));
            }
        }

        answers.sort_by(|left, right| {
            right.weight.cmp(&left.weight).then(left.answer.value.cmp(&right.answer.value))
        });
        answers.dedup_by(|left, right| {
            left.answer.record_type == right.answer.record_type
                && left.answer.value == right.answer.value
        });

        if answers.is_empty() {
            self.response_nxdomain_total.fetch_add(1, Ordering::Relaxed);
            self.record_query_stat(
                &snapshot.cluster_id,
                Some(&zone_name),
                &qname,
                record_type,
                ResponseCode::NXDomain,
                0,
            );
            DnsQueryResult::nxdomain()
        } else {
            self.response_noerror_total.fetch_add(1, Ordering::Relaxed);
            let answer_count = answers.len();
            self.record_query_stat(
                &snapshot.cluster_id,
                Some(&zone_name),
                &qname,
                record_type,
                ResponseCode::NoError,
                answer_count,
            );
            DnsQueryResult::noerror(
                record.ttl_secs,
                answers.into_iter().map(|answer| answer.answer).collect(),
            )
        }
    }
}

pub async fn serve(
    config: DnsServerConfig,
    engine: Arc<dyn DnsQueryEngine>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut tasks = Vec::new();
    if let Some(udp_bind_addr) = config.udp_bind_addr {
        let engine = engine.clone();
        let shutdown = shutdown.clone();
        tasks.push(tokio::spawn(async move { run_udp(udp_bind_addr, engine, shutdown).await }));
    }
    if let Some(tcp_bind_addr) = config.tcp_bind_addr {
        let engine = engine.clone();
        let shutdown = shutdown.clone();
        tasks.push(tokio::spawn(async move { run_tcp(tcp_bind_addr, engine, shutdown).await }));
    }

    for task in tasks {
        task.await.context("dns listener task failed to join")??;
    }
    Ok(())
}

async fn run_udp(
    bind_addr: SocketAddr,
    engine: Arc<dyn DnsQueryEngine>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let socket = UdpSocket::bind(bind_addr)
        .await
        .with_context(|| format!("failed to bind udp dns listener on {bind_addr}"))?;
    tracing::info!(bind_addr = %bind_addr, transport = "udp", "authoritative dns listener ready");

    let mut buffer = vec![0_u8; 4096];
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            received = socket.recv_from(&mut buffer) => {
                let (size, remote_addr) = match received {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::warn!(bind_addr = %bind_addr, error = %error, "udp dns recv failed");
                        continue;
                    }
                };
                if let Some(response) = handle_dns_bytes(&buffer[..size], remote_addr.ip(), engine.as_ref()) {
                    let _ = socket.send_to(&response, remote_addr).await;
                }
            }
        }
    }

    Ok(())
}

async fn run_tcp(
    bind_addr: SocketAddr,
    engine: Arc<dyn DnsQueryEngine>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("failed to bind tcp dns listener on {bind_addr}"))?;
    tracing::info!(bind_addr = %bind_addr, transport = "tcp", "authoritative dns listener ready");

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let (stream, remote_addr) = match accepted {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::warn!(bind_addr = %bind_addr, error = %error, "tcp dns accept failed");
                        continue;
                    }
                };
                let engine = engine.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_tcp_connection(stream, remote_addr.ip(), engine).await {
                        tracing::warn!(remote_addr = %remote_addr, error = %error, "tcp dns session failed");
                    }
                });
            }
        }
    }

    Ok(())
}

async fn handle_tcp_connection(
    mut stream: TcpStream,
    source_ip: IpAddr,
    engine: Arc<dyn DnsQueryEngine>,
) -> Result<()> {
    loop {
        let length = match stream.read_u16().await {
            Ok(length) => length,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error).context("failed to read tcp dns frame length"),
        };
        let mut buffer = vec![0_u8; usize::from(length)];
        stream.read_exact(&mut buffer).await.context("failed to read tcp dns frame body")?;
        let Some(response) = handle_dns_bytes(&buffer, source_ip, engine.as_ref()) else {
            continue;
        };
        let response_len =
            u16::try_from(response.len()).context("tcp dns response length should fit into u16")?;
        stream.write_u16(response_len).await.context("failed to write tcp dns response length")?;
        stream.write_all(&response).await.context("failed to write tcp dns response body")?;
        stream.flush().await.context("failed to flush tcp dns response")?;
    }
}

fn handle_dns_bytes(
    bytes: &[u8],
    source_ip: IpAddr,
    engine: &dyn DnsQueryEngine,
) -> Option<Vec<u8>> {
    let request = Message::from_vec(bytes).ok()?;
    let mut response = Message::new();
    response.set_id(request.id());
    response.set_message_type(MessageType::Response);
    response.set_op_code(OpCode::Query);
    response.set_recursion_desired(request.recursion_desired());
    response.set_recursion_available(false);
    response.set_authoritative(true);

    let Some(query) = request.queries().first().cloned() else {
        response.set_response_code(ResponseCode::FormErr);
        return encode_message(&response).ok();
    };
    response.add_query(query.clone());
    let Some(record_type) = map_record_type(query.query_type()) else {
        response.set_response_code(ResponseCode::NotImp);
        return encode_message(&response).ok();
    };

    let qname = query.name().to_ascii();
    let result = engine.resolve(qname.trim_end_matches('.'), record_type, source_ip);
    response.set_response_code(result.response_code);
    if result.response_code == ResponseCode::NoError {
        for answer in result.answers {
            if let Some(record) = build_answer_record(query.name().clone(), result.ttl_secs, answer)
            {
                response.add_answer(record);
            }
        }
    }
    encode_message(&response).ok()
}

fn encode_message(message: &Message) -> Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(1024);
    let mut encoder = BinEncoder::new(&mut buffer);
    message.emit(&mut encoder).context("failed to encode dns message")?;
    Ok(buffer)
}

fn map_record_type(record_type: RecordType) -> Option<DnsRecordType> {
    match record_type {
        RecordType::A => Some(DnsRecordType::A),
        RecordType::AAAA => Some(DnsRecordType::Aaaa),
        RecordType::CNAME => Some(DnsRecordType::Cname),
        RecordType::TXT => Some(DnsRecordType::Txt),
        _ => None,
    }
}

fn build_answer_record(name: Name, ttl_secs: u32, answer: DnsAnswer) -> Option<Record> {
    let data = match answer.record_type {
        DnsRecordType::A => {
            Some(RData::A(A::from(answer.value.parse::<std::net::Ipv4Addr>().ok()?)))
        }
        DnsRecordType::Aaaa => {
            Some(RData::AAAA(AAAA::from(answer.value.parse::<std::net::Ipv6Addr>().ok()?)))
        }
        DnsRecordType::Cname => Some(RData::CNAME(CNAME(Name::from_ascii(answer.value).ok()?))),
        DnsRecordType::Txt => Some(RData::TXT(TXT::new(vec![answer.value]))),
    }?;
    Some(Record::from_rdata(name, ttl_secs, data))
}

impl CompiledDnsSnapshot {
    fn from_snapshot(snapshot: DnsPublishedSnapshot) -> Self {
        let nodes_by_id = snapshot
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.node_id.clone(), node))
            .collect::<HashMap<_, _>>();
        let cluster_nodes = snapshot.nodes.clone();
        let resolved_upstreams = snapshot
            .resolved_upstreams
            .into_iter()
            .map(|(upstream, addrs)| {
                let parsed = addrs
                    .into_iter()
                    .filter_map(|addr| addr.parse::<IpAddr>().ok())
                    .collect::<Vec<_>>();
                (upstream, parsed)
            })
            .collect::<HashMap<_, _>>();

        Self {
            cluster_id: snapshot.cluster_id,
            revision_id: snapshot.revision_id,
            version_label: snapshot.version_label,
            plan: snapshot.plan,
            nodes_by_id,
            cluster_nodes,
            resolved_upstreams,
        }
    }
}

fn prune_query_stats(stats: &mut HashMap<QueryStatKey, QueryStatValue>) {
    let retain_keys = stats
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.retention_score(),
                value.error_total(),
                value.last_query_at_unix_ms,
                value.query_total,
            )
        })
        .collect::<Vec<_>>();
    let mut ranked = retain_keys;
    ranked.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then(right.2.cmp(&left.2))
            .then(right.3.cmp(&left.3))
            .then(right.4.cmp(&left.4))
            .then(left.0.qname.cmp(&right.0.qname))
            .then(left.0.record_type.as_str().cmp(right.0.record_type.as_str()))
    });
    let keep = ranked
        .into_iter()
        .take(DNS_RUNTIME_QUERY_STAT_LIMIT)
        .map(|item| item.0)
        .collect::<HashSet<_>>();
    stats.retain(|key, _| keep.contains(key));
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}

fn normalize_name(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn record_fqdn(zone: &rginx_control_types::DnsZoneSpec, record_name: &str) -> String {
    let record_name = record_name.trim();
    if record_name.is_empty() || record_name == "@" {
        normalize_name(&zone.zone_name)
    } else if record_name.ends_with('.') {
        normalize_name(record_name)
    } else {
        format!("{}.{}", normalize_name(record_name), normalize_name(&zone.zone_name))
    }
}

fn ip_matches_record_type(ip: IpAddr, record_type: DnsRecordType) -> bool {
    matches!(
        (ip, record_type),
        (IpAddr::V4(_), DnsRecordType::A) | (IpAddr::V6(_), DnsRecordType::Aaaa)
    )
}

fn advertise_ip(advertise_addr: &str) -> Option<IpAddr> {
    advertise_addr.to_socket_addrs().ok().and_then(|mut addrs| addrs.next()).map(|addr| addr.ip())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use rginx_control_types::{
        DnsAnswerTarget, DnsPlan, DnsPublishedSnapshot, DnsRecordSet, DnsRecordType, DnsTargetKind,
        DnsZoneSpec, NodeLifecycleState, NodeSummary,
    };

    use super::{DnsQueryEngine, InMemoryDnsRuntime};

    #[test]
    fn resolves_weighted_answers_with_source_cidrs() {
        let runtime = InMemoryDnsRuntime::new(None, None);
        runtime.replace_snapshots(vec![DnsPublishedSnapshot {
            cluster_id: "cluster-mainland".to_string(),
            revision_id: "dns_rev_1".to_string(),
            version_label: "v1".to_string(),
            plan: DnsPlan {
                cluster_id: "cluster-mainland".to_string(),
                zones: vec![DnsZoneSpec {
                    zone_id: "zone-main".to_string(),
                    zone_name: "example.com".to_string(),
                    records: vec![DnsRecordSet {
                        record_id: "record-www-a".to_string(),
                        name: "www".to_string(),
                        record_type: DnsRecordType::A,
                        ttl_secs: 30,
                        values: Vec::new(),
                        targets: vec![
                            DnsAnswerTarget {
                                target_id: "cluster".to_string(),
                                kind: DnsTargetKind::Cluster,
                                value: "cluster-mainland".to_string(),
                                weight: 20,
                                enabled: true,
                                source_cidrs: Vec::new(),
                                tags: Vec::new(),
                            },
                            DnsAnswerTarget {
                                target_id: "source-specific".to_string(),
                                kind: DnsTargetKind::StaticIp,
                                value: "198.51.100.5".to_string(),
                                weight: 50,
                                enabled: true,
                                source_cidrs: vec!["198.51.100.0/24".to_string()],
                                tags: Vec::new(),
                            },
                            DnsAnswerTarget {
                                target_id: "mismatch".to_string(),
                                kind: DnsTargetKind::StaticIp,
                                value: "203.0.113.5".to_string(),
                                weight: 100,
                                enabled: true,
                                source_cidrs: vec!["203.0.113.0/24".to_string()],
                                tags: Vec::new(),
                            },
                        ],
                    }],
                }],
            },
            nodes: vec![
                test_node("node-a", "cluster-mainland", "192.0.2.10:443"),
                test_node("node-b", "cluster-mainland", "192.0.2.11:443"),
            ],
            resolved_upstreams: BTreeMap::new(),
        }]);

        let result = runtime.resolve(
            "www.example.com",
            DnsRecordType::A,
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9)),
        );

        assert_eq!(
            result.answers.iter().map(|answer| answer.value.as_str()).collect::<Vec<_>>(),
            vec!["198.51.100.5", "192.0.2.10", "192.0.2.11"]
        );
    }

    #[test]
    fn filters_targets_by_record_family() {
        let runtime = InMemoryDnsRuntime::new(None, None);
        runtime.replace_snapshots(vec![DnsPublishedSnapshot {
            cluster_id: "cluster-mainland".to_string(),
            revision_id: "dns_rev_2".to_string(),
            version_label: "v2".to_string(),
            plan: DnsPlan {
                cluster_id: "cluster-mainland".to_string(),
                zones: vec![DnsZoneSpec {
                    zone_id: "zone-main".to_string(),
                    zone_name: "example.com".to_string(),
                    records: vec![DnsRecordSet {
                        record_id: "record-v6".to_string(),
                        name: "edge".to_string(),
                        record_type: DnsRecordType::Aaaa,
                        ttl_secs: 60,
                        values: Vec::new(),
                        targets: vec![
                            DnsAnswerTarget {
                                target_id: "node-v6".to_string(),
                                kind: DnsTargetKind::Node,
                                value: "node-v6".to_string(),
                                weight: 10,
                                enabled: true,
                                source_cidrs: Vec::new(),
                                tags: Vec::new(),
                            },
                            DnsAnswerTarget {
                                target_id: "origin".to_string(),
                                kind: DnsTargetKind::Upstream,
                                value: "origin".to_string(),
                                weight: 5,
                                enabled: true,
                                source_cidrs: Vec::new(),
                                tags: Vec::new(),
                            },
                        ],
                    }],
                }],
            },
            nodes: vec![test_node("node-v6", "cluster-mainland", "[2001:db8::10]:443")],
            resolved_upstreams: BTreeMap::from([(
                "origin".to_string(),
                vec!["2001:db8::20".to_string(), "192.0.2.20".to_string()],
            )]),
        }]);

        let result = runtime.resolve(
            "edge.example.com",
            DnsRecordType::Aaaa,
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 2)),
        );

        assert_eq!(
            result.answers.iter().map(|answer| answer.value.as_str()).collect::<Vec<_>>(),
            vec!["2001:db8::10", "2001:db8::20"]
        );
    }

    #[test]
    fn cluster_status_reports_bind_addresses_without_snapshot() {
        let runtime = InMemoryDnsRuntime::new(
            Some(SocketAddr::from(([127, 0, 0, 1], 5353))),
            Some(SocketAddr::from(([127, 0, 0, 1], 5354))),
        );

        let result = runtime.resolve(
            "missing.example.com",
            DnsRecordType::A,
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        );
        assert_eq!(result.response_code, hickory_proto::op::ResponseCode::NXDomain);

        let status = runtime.cluster_status("cluster-mainland");
        assert!(status.enabled);
        assert_eq!(status.cluster_id, "cluster-mainland");
        assert_eq!(status.udp_bind_addr.as_deref(), Some("127.0.0.1:5353"));
        assert_eq!(status.tcp_bind_addr.as_deref(), Some("127.0.0.1:5354"));
        assert_eq!(status.published_revision_id, None);
        assert_eq!(status.query_total, 1);
        assert_eq!(status.response_nxdomain_total, 1);
        assert!(status.hot_queries.is_empty());
        assert!(status.error_queries.is_empty());
    }

    #[test]
    fn cluster_status_reports_hot_queries_and_errors() {
        let runtime = InMemoryDnsRuntime::new(None, None);
        runtime.replace_snapshots(vec![DnsPublishedSnapshot {
            cluster_id: "cluster-mainland".to_string(),
            revision_id: "dns_rev_hot".to_string(),
            version_label: "hot-v1".to_string(),
            plan: DnsPlan {
                cluster_id: "cluster-mainland".to_string(),
                zones: vec![DnsZoneSpec {
                    zone_id: "zone-main".to_string(),
                    zone_name: "example.com".to_string(),
                    records: vec![DnsRecordSet {
                        record_id: "record-www-a".to_string(),
                        name: "www".to_string(),
                        record_type: DnsRecordType::A,
                        ttl_secs: 30,
                        values: vec!["192.0.2.30".to_string()],
                        targets: Vec::new(),
                    }],
                }],
            },
            nodes: vec![test_node("node-a", "cluster-mainland", "192.0.2.10:443")],
            resolved_upstreams: BTreeMap::new(),
        }]);

        let _ = runtime.resolve(
            "www.example.com",
            DnsRecordType::A,
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9)),
        );
        let _ = runtime.resolve(
            "www.example.com",
            DnsRecordType::A,
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10)),
        );
        let _ = runtime.resolve(
            "missing.example.com",
            DnsRecordType::A,
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 11)),
        );

        let status = runtime.cluster_status("cluster-mainland");
        assert_eq!(status.query_total, 3);
        assert_eq!(status.response_noerror_total, 2);
        assert_eq!(status.response_nxdomain_total, 1);
        assert_eq!(status.hot_queries.len(), 2);
        assert_eq!(status.hot_queries[0].qname, "www.example.com");
        assert_eq!(status.hot_queries[0].zone_name.as_deref(), Some("example.com"));
        assert_eq!(status.hot_queries[0].query_total, 2);
        assert_eq!(status.hot_queries[0].answer_total, 2);
        assert_eq!(status.hot_queries[0].response_noerror_total, 2);
        assert_eq!(status.error_queries.len(), 1);
        assert_eq!(status.error_queries[0].qname, "missing.example.com");
        assert_eq!(status.error_queries[0].response_nxdomain_total, 1);
    }

    fn test_node(node_id: &str, cluster_id: &str, advertise_addr: &str) -> NodeSummary {
        NodeSummary {
            node_id: node_id.to_string(),
            cluster_id: cluster_id.to_string(),
            advertise_addr: advertise_addr.to_string(),
            role: "edge".to_string(),
            state: NodeLifecycleState::Online,
            running_version: "test".to_string(),
            admin_socket_path: "/tmp/rginx.sock".to_string(),
            last_seen_unix_ms: 0,
            last_snapshot_version: None,
            runtime_revision: None,
            runtime_pid: None,
            listener_count: None,
            active_connections: None,
            status_reason: None,
        }
    }
}
