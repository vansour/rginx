use super::*;

pub(super) struct CacheZoneRuntime {
    pub(super) config: Arc<CacheZone>,
    pub(super) index: RwLock<CacheIndex>,
    pub(super) hot_entries: RwLock<HashMap<String, Arc<CacheEntryHotState>>>,
    pub(super) io_locks: CacheIoLockPool,
    pub(super) shared_index_sync_lock: AsyncMutex<()>,
    pub(super) shared_index_store: Option<Arc<SharedIndexStore>>,
    pub(super) fill_locks: Arc<Mutex<HashMap<String, CacheFillLockState>>>,
    pub(super) fill_lock_generation: AtomicU64,
    pub(super) last_inactive_cleanup_unix_ms: AtomicU64,
    pub(super) shared_index_generation: AtomicU64,
    pub(super) shared_index_store_epoch: AtomicU64,
    pub(super) shared_index_change_seq: AtomicU64,
    pub(super) stats: CacheZoneStats,
    pub(super) change_notifier: Option<CacheChangeNotifier>,
}

#[derive(Default, Clone)]
pub(super) struct CacheIndex {
    pub(super) entries: HashMap<String, CacheIndexEntry>,
    pub(super) hash_ref_counts: HashMap<String, usize>,
    pub(super) variants: HashMap<String, Vec<String>>,
    pub(super) admission_counts: HashMap<String, u64>,
    pub(super) invalidations: Vec<CacheInvalidationRule>,
    pub(super) current_size_bytes: usize,
    pub(super) maintenance_next_ticket: u64,
    pub(super) access_schedule: BTreeSet<CacheAccessScheduleEntry>,
    pub(super) access_ticket_by_key: HashMap<String, CacheAccessScheduleTicket>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct CacheAccessScheduleEntry {
    pub(super) last_access_unix_ms: u64,
    pub(super) ticket: u64,
    pub(super) key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheAccessScheduleTicket {
    pub(super) last_access_unix_ms: u64,
    pub(super) ticket: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CacheIndexEntry {
    pub(super) kind: CacheIndexEntryKind,
    pub(super) hash: String,
    pub(super) base_key: String,
    pub(super) stored_at_unix_ms: u64,
    pub(super) vary: Vec<CachedVaryHeaderValue>,
    pub(super) tags: Vec<String>,
    pub(super) body_size_bytes: usize,
    pub(super) expires_at_unix_ms: u64,
    pub(super) grace_until_unix_ms: Option<u64>,
    pub(super) keep_until_unix_ms: Option<u64>,
    pub(super) stale_if_error_until_unix_ms: Option<u64>,
    pub(super) stale_while_revalidate_until_unix_ms: Option<u64>,
    pub(super) requires_revalidation: bool,
    pub(super) must_revalidate: bool,
    pub(super) last_access_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum CacheIndexEntryKind {
    #[default]
    Response,
    HitForPass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CacheInvalidationSelector {
    All,
    Exact(String),
    Prefix(String),
    Tag(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CacheInvalidationRule {
    pub(super) selector: CacheInvalidationSelector,
    pub(super) created_at_unix_ms: u64,
}

#[derive(Default)]
pub(super) struct CacheZoneStats {
    pub(super) hit_total: AtomicU64,
    pub(super) miss_total: AtomicU64,
    pub(super) bypass_total: AtomicU64,
    pub(super) expired_total: AtomicU64,
    pub(super) stale_total: AtomicU64,
    pub(super) updating_total: AtomicU64,
    pub(super) revalidated_total: AtomicU64,
    pub(super) write_success_total: AtomicU64,
    pub(super) write_error_total: AtomicU64,
    pub(super) eviction_total: AtomicU64,
    pub(super) purge_total: AtomicU64,
    pub(super) invalidation_total: AtomicU64,
    pub(super) inactive_cleanup_total: AtomicU64,
}

pub(super) struct CacheFillGuard {
    pub(super) key: String,
    pub(super) generation: u64,
    pub(super) fill_locks: Weak<Mutex<HashMap<String, CacheFillLockState>>>,
    pub(super) notify: Arc<Notify>,
    pub(super) external_lock_path: Option<PathBuf>,
    pub(super) external_state: Option<SharedFillExternalStateHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CachedVaryHeaderValue {
    pub(super) name: http::header::HeaderName,
    pub(super) value: Option<String>,
}

#[derive(Clone)]
pub(super) struct CacheFillLockState {
    pub(super) notify: Arc<Notify>,
    pub(super) acquired_at_unix_ms: u64,
    pub(super) generation: u64,
    pub(super) share_fingerprint: String,
    pub(super) reader_state: Option<Arc<CacheFillReadState>>,
}

#[derive(Clone)]
pub(super) struct CacheConditionalHeaders {
    pub(super) if_none_match: Option<HeaderValue>,
    pub(super) if_modified_since: Option<HeaderValue>,
}

pub(super) struct CacheEntryHotState {
    pub(super) last_access_unix_ms: AtomicU64,
    pub(super) response_head: Mutex<Option<Arc<PreparedCacheResponseHead>>>,
}

pub(super) struct PreparedCacheResponseHead {
    pub(super) hash: String,
    pub(super) metadata: Arc<CacheMetadata>,
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) conditional_headers: Option<CacheConditionalHeaders>,
}

pub(super) enum LookupDecision {
    Bypass {
        status: CacheStatus,
    },
    DropEntry {
        key: String,
        entry: CacheIndexEntry,
    },
    FreshHit {
        key: String,
        entry: CacheIndexEntry,
    },
    Stale {
        key: String,
        entry: CacheIndexEntry,
        status: CacheStatus,
    },
    BackgroundUpdate {
        key: String,
        cached_entry: CacheIndexEntry,
        fill_guard: CacheFillGuard,
    },
    Miss {
        key: String,
        base_key: String,
        cached_entry: Option<CacheIndexEntry>,
        fill_guard: Option<CacheFillGuard>,
        cache_status: CacheStatus,
    },
    ReadWhileFillLocal {
        state: Arc<CacheFillReadState>,
    },
    ReadWhileFillExternal {
        state: ExternalCacheFillReadState,
    },
    Wait {
        strategy: LookupWait,
    },
}

pub(super) enum LookupWait {
    Local { waiter: OwnedNotified },
    External { key: String },
}

pub(super) enum FillLockDecision {
    Acquired(CacheFillGuard),
    ReadLocal { state: Arc<CacheFillReadState> },
    ReadExternal { state: ExternalCacheFillReadState },
    WaitLocal { waiter: OwnedNotified },
    WaitExternal { key: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheStaleReason {
    Error,
    Timeout,
    Status(StatusCode),
}
