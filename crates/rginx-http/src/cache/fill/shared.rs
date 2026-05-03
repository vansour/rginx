use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use serde::{Deserialize, Serialize};

use super::super::entry::unix_time_ms;
use super::super::shared::{
    SharedFillLockAcquire, SharedFillLockStatus, SharedIndexStore, run_blocking,
    shared_fill_lock_path, shared_fill_state_path,
};
use super::persistence::{atomic_write_json, next_shared_fill_nonce};

#[derive(Clone)]
pub(in crate::cache) struct SharedFillExternalStateHandle {
    backend: SharedFillStateBackend,
    state: Arc<Mutex<SharedFillStateRecord>>,
}

#[derive(Clone)]
enum SharedFillStateBackend {
    File { lock_path: PathBuf, state_path: PathBuf },
    SharedMemory { store: Arc<SharedIndexStore>, key: String },
}

#[derive(Clone)]
pub(in crate::cache) struct ExternalCacheFillReadState {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body_tmp_path: PathBuf,
    pub(super) body_path: PathBuf,
    pub(super) source: ExternalFillStateSource,
}

#[derive(Clone)]
pub(super) enum ExternalFillStateSource {
    File { state_path: PathBuf, nonce: String },
    SharedMemory { store: Arc<SharedIndexStore>, key: String, nonce: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedFillLockRecord {
    nonce: String,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SharedFillStateRecord {
    pub(super) nonce: String,
    #[serde(default)]
    pub(super) share_fingerprint: String,
    pub(super) response: Option<SharedFillResponseMetadata>,
    pub(super) upstream_completed: bool,
    pub(super) finished: bool,
    pub(super) trailers: Option<Vec<SharedFillHeader>>,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SharedFillResponseMetadata {
    status: u16,
    headers: Vec<SharedFillHeader>,
    body_tmp_path: PathBuf,
    body_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SharedFillHeader {
    name: String,
    value: Vec<u8>,
}

impl SharedFillExternalStateHandle {
    fn create_file(
        zone: &rginx_core::CacheZone,
        key: &str,
        lock_path: &Path,
        now: u64,
        share_fingerprint: Option<&str>,
    ) -> std::io::Result<Self> {
        let state_path = shared_fill_state_path(zone, key);
        let state = SharedFillStateRecord {
            nonce: next_shared_fill_nonce(now),
            share_fingerprint: share_fingerprint.unwrap_or_default().to_string(),
            response: None,
            upstream_completed: false,
            finished: false,
            trailers: None,
            error: None,
        };
        let handle = Self {
            backend: SharedFillStateBackend::File {
                lock_path: lock_path.to_path_buf(),
                state_path,
            },
            state: Arc::new(Mutex::new(state)),
        };
        handle.persist_lock_and_state(now)?;
        Ok(handle)
    }

    fn create_shared_memory(
        store: Arc<SharedIndexStore>,
        key: &str,
        now: u64,
        lock_age: std::time::Duration,
        share_fingerprint: Option<&str>,
    ) -> std::io::Result<Option<Self>> {
        let state = SharedFillStateRecord {
            nonce: next_shared_fill_nonce(now),
            share_fingerprint: share_fingerprint.unwrap_or_default().to_string(),
            response: None,
            upstream_completed: false,
            finished: false,
            trailers: None,
            error: None,
        };
        let state_json = shared_fill_state_record_bytes(&state)?;
        let acquire = store.try_acquire_fill_lock(
            key,
            now,
            lock_age.as_millis() as u64,
            &state.nonce,
            &state_json,
        )?;
        match acquire {
            SharedFillLockAcquire::Acquired => Ok(Some(Self {
                backend: SharedFillStateBackend::SharedMemory { store, key: key.to_string() },
                state: Arc::new(Mutex::new(state)),
            })),
            SharedFillLockAcquire::Busy => Ok(None),
        }
    }

    pub(super) fn publish_response(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        body_tmp_path: &Path,
        body_path: &Path,
    ) -> std::io::Result<()> {
        self.update_state(|state| {
            state.response = Some(SharedFillResponseMetadata {
                status: status.as_u16(),
                headers: shared_headers_from_map(headers),
                body_tmp_path: body_tmp_path.to_path_buf(),
                body_path: body_path.to_path_buf(),
            });
        })
    }

    pub(super) fn mark_upstream_complete(&self) -> std::io::Result<()> {
        self.update_state(|state| {
            state.upstream_completed = true;
        })
    }

    pub(super) fn finish(&self, trailers: Option<HeaderMap>) -> std::io::Result<()> {
        self.update_state(|state| {
            state.upstream_completed = true;
            state.finished = true;
            state.trailers = trailers.as_ref().map(shared_headers_from_map);
        })
    }

    pub(super) fn fail(&self, error: impl std::fmt::Display) -> std::io::Result<()> {
        let error = error.to_string();
        self.update_state(move |state| {
            state.error = Some(error.clone());
        })
    }

    pub(super) fn heartbeat(&self) -> std::io::Result<()> {
        run_blocking(|| {
            let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match &self.backend {
                SharedFillStateBackend::File { lock_path, .. } => persist_shared_fill_lock_record(
                    lock_path,
                    &SharedFillLockRecord {
                        nonce: state.nonce.clone(),
                        updated_at_unix_ms: unix_time_ms(SystemTime::now()),
                    },
                ),
                SharedFillStateBackend::SharedMemory { store, key } => store.update_fill_lock(
                    key,
                    &state.nonce,
                    unix_time_ms(SystemTime::now()),
                    &shared_fill_state_record_bytes(&state)?,
                ),
            }
        })
    }

    fn persist_lock_and_state(&self, now: u64) -> std::io::Result<()> {
        run_blocking(|| {
            let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match &self.backend {
                SharedFillStateBackend::File { lock_path, state_path } => {
                    persist_shared_fill_state_record(state_path, &state)?;
                    persist_shared_fill_lock_record(
                        lock_path,
                        &SharedFillLockRecord {
                            nonce: state.nonce.clone(),
                            updated_at_unix_ms: now,
                        },
                    )
                }
                SharedFillStateBackend::SharedMemory { store, key } => store.update_fill_lock(
                    key,
                    &state.nonce,
                    now,
                    &shared_fill_state_record_bytes(&state)?,
                ),
            }
        })
    }

    fn update_state(&self, update: impl FnOnce(&mut SharedFillStateRecord)) -> std::io::Result<()> {
        run_blocking(|| {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            update(&mut state);
            match &self.backend {
                SharedFillStateBackend::File { lock_path, state_path } => {
                    persist_shared_fill_state_record(state_path, &state)?;
                    persist_shared_fill_lock_record(
                        lock_path,
                        &SharedFillLockRecord {
                            nonce: state.nonce.clone(),
                            updated_at_unix_ms: unix_time_ms(SystemTime::now()),
                        },
                    )
                }
                SharedFillStateBackend::SharedMemory { store, key } => store.update_fill_lock(
                    key,
                    &state.nonce,
                    unix_time_ms(SystemTime::now()),
                    &shared_fill_state_record_bytes(&state)?,
                ),
            }
        })
    }

    pub(in crate::cache) fn release(&self) {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match &self.backend {
            SharedFillStateBackend::File { lock_path, .. } => {
                let _ = std::fs::remove_file(lock_path);
            }
            SharedFillStateBackend::SharedMemory { store, key } => {
                let _ = store.release_fill_lock(key, &state.nonce);
            }
        }
    }
}

pub(in crate::cache) fn create_file_shared_external_fill_handle(
    zone: &rginx_core::CacheZone,
    key: &str,
    lock_path: &Path,
    now: u64,
    share_fingerprint: Option<&str>,
) -> std::io::Result<SharedFillExternalStateHandle> {
    SharedFillExternalStateHandle::create_file(zone, key, lock_path, now, share_fingerprint)
}

pub(in crate::cache) fn create_memory_shared_external_fill_handle(
    store: Arc<SharedIndexStore>,
    key: &str,
    now: u64,
    lock_age: std::time::Duration,
    share_fingerprint: Option<&str>,
) -> std::io::Result<Option<SharedFillExternalStateHandle>> {
    SharedFillExternalStateHandle::create_shared_memory(
        store,
        key,
        now,
        lock_age,
        share_fingerprint,
    )
}

pub(in crate::cache) fn load_file_external_fill_state(
    zone: &rginx_core::CacheZone,
    key: &str,
    share_fingerprint: Option<&str>,
) -> Option<ExternalCacheFillReadState> {
    let lock_path = shared_fill_lock_path(zone, key);
    let state_path = shared_fill_state_path(zone, key);
    let lock = read_shared_fill_lock_record(&lock_path).ok()?;
    let state = read_shared_fill_state_record(&state_path).ok()?;
    if state.nonce != lock.nonce {
        return None;
    }
    if share_fingerprint.is_some_and(|fingerprint| state.share_fingerprint != fingerprint) {
        return None;
    }
    if state.error.is_some() || state.upstream_completed {
        return None;
    }
    let response = state.response?;
    let status = StatusCode::from_u16(response.status).ok()?;
    let headers = header_map_from_shared_headers(&response.headers).ok()?;
    Some(ExternalCacheFillReadState {
        status,
        headers,
        body_tmp_path: response.body_tmp_path,
        body_path: response.body_path,
        source: ExternalFillStateSource::File { state_path, nonce: state.nonce },
    })
}

pub(in crate::cache) fn load_memory_external_fill_state(
    store: Arc<SharedIndexStore>,
    key: &str,
    share_fingerprint: Option<&str>,
) -> Option<ExternalCacheFillReadState> {
    let snapshot = store.load_fill_lock(key).ok()??;
    let state = shared_fill_state_record_from_bytes(&snapshot.state_json).ok()?;
    if state.nonce != snapshot.nonce {
        return None;
    }
    if share_fingerprint.is_some_and(|fingerprint| state.share_fingerprint != fingerprint) {
        return None;
    }
    if state.error.is_some() || state.upstream_completed {
        return None;
    }
    let response = state.response?;
    let status = StatusCode::from_u16(response.status).ok()?;
    let headers = header_map_from_shared_headers(&response.headers).ok()?;
    Some(ExternalCacheFillReadState {
        status,
        headers,
        body_tmp_path: response.body_tmp_path,
        body_path: response.body_path,
        source: ExternalFillStateSource::SharedMemory {
            store,
            key: key.to_string(),
            nonce: state.nonce,
        },
    })
}

pub(in crate::cache) fn memory_shared_fill_lock_state(
    store: &SharedIndexStore,
    key: &str,
    now: u64,
    lock_age: std::time::Duration,
) -> std::io::Result<SharedFillLockStatus> {
    store.fill_lock_status(key, now, lock_age.as_millis() as u64)
}

pub(in crate::cache) fn clear_stale_memory_shared_fill_lock(
    store: &SharedIndexStore,
    key: &str,
    now: u64,
    lock_age: std::time::Duration,
) -> std::io::Result<bool> {
    store.clear_stale_fill_lock(key, now, lock_age.as_millis() as u64)
}

pub(super) fn read_external_fill_state_record(
    source: &ExternalFillStateSource,
) -> std::io::Result<SharedFillStateRecord> {
    let (state, nonce) = match source {
        ExternalFillStateSource::File { state_path, nonce } => {
            (read_shared_fill_state_record(state_path)?, nonce.as_str())
        }
        ExternalFillStateSource::SharedMemory { store, key, nonce } => {
            let snapshot = store.load_fill_lock(key)?.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "shared fill lock not found")
            })?;
            (shared_fill_state_record_from_bytes(&snapshot.state_json)?, nonce.as_str())
        }
    };
    if state.nonce != nonce {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "shared fill state nonce mismatch: expected `{}`, found `{}`",
                nonce, state.nonce
            ),
        ));
    }
    Ok(state)
}

pub(super) fn header_map_from_shared_headers(
    headers: &[SharedFillHeader],
) -> std::io::Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for header in headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let value = HeaderValue::from_bytes(&header.value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        map.append(name, value);
    }
    Ok(map)
}

fn shared_headers_from_map(headers: &HeaderMap) -> Vec<SharedFillHeader> {
    headers
        .iter()
        .map(|(name, value)| SharedFillHeader {
            name: name.as_str().to_string(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

fn shared_fill_state_record_bytes(record: &SharedFillStateRecord) -> std::io::Result<Vec<u8>> {
    serde_json::to_vec(record)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn shared_fill_state_record_from_bytes(bytes: &[u8]) -> std::io::Result<SharedFillStateRecord> {
    serde_json::from_slice(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_shared_fill_lock_record(path: &Path) -> std::io::Result<SharedFillLockRecord> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_shared_fill_state_record(path: &Path) -> std::io::Result<SharedFillStateRecord> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn persist_shared_fill_lock_record(
    path: &Path,
    record: &SharedFillLockRecord,
) -> std::io::Result<()> {
    atomic_write_json(path, record)
}

fn persist_shared_fill_state_record(
    path: &Path,
    record: &SharedFillStateRecord,
) -> std::io::Result<()> {
    atomic_write_json(path, record)
}
