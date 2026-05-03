use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::*;

enum FileSharedFillLockState {
    Missing,
    Fresh,
    Stale,
}

impl CacheZoneRuntime {
    pub(in crate::cache) fn fill_lock_decision(
        self: &Arc<Self>,
        key: &str,
        now: u64,
        lock_age: std::time::Duration,
        share_fingerprint: Option<&str>,
    ) -> FillLockDecision {
        let mut fill_locks =
            self.fill_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(lock) = fill_locks.get(key).cloned()
            && now.saturating_sub(lock.acquired_at_unix_ms) <= lock_age.as_millis() as u64
        {
            if share_fingerprint.is_none_or(|fingerprint| lock.share_fingerprint == fingerprint)
                && let Some(state) = lock.reader_state.filter(|state| state.can_share())
            {
                return FillLockDecision::ReadLocal { state };
            }
            return FillLockDecision::WaitLocal { waiter: lock.notify.notified_owned() };
        }

        let (external_lock_path, external_state) = if self.config.shared_index {
            if let Some(store) = self.shared_index_store.as_ref()
                && store.supports_shared_fill_locks()
            {
                match super::super::fill::create_memory_shared_external_fill_handle(
                    store.clone(),
                    key,
                    now,
                    lock_age,
                    share_fingerprint,
                ) {
                    Ok(Some(state)) => (None, Some(state)),
                    Ok(None) => {
                        if let Some(state) = super::super::fill::load_memory_external_fill_state(
                            store.clone(),
                            key,
                            share_fingerprint,
                        ) {
                            return FillLockDecision::ReadExternal { state };
                        }
                        return FillLockDecision::WaitExternal { key: key.to_string() };
                    }
                    Err(error) => {
                        tracing::warn!(
                            zone = %self.config.name,
                            key = %key,
                            path = %store.path().display(),
                            %error,
                            "failed to initialize shm shared fill state; falling back to local fill coordination"
                        );
                        (None, None)
                    }
                }
            } else {
                match self.try_acquire_file_shared_fill_lock(key, lock_age) {
                    Some(lock_path) => {
                        match super::super::fill::create_file_shared_external_fill_handle(
                            self.config.as_ref(),
                            key,
                            &lock_path,
                            now,
                            share_fingerprint,
                        ) {
                            Ok(state) => (Some(lock_path), Some(state)),
                            Err(error) => {
                                tracing::warn!(
                                    zone = %self.config.name,
                                    key = %key,
                                    path = %lock_path.display(),
                                    %error,
                                    "failed to initialize external shared fill state; falling back to local fill coordination"
                                );
                                let _ = std::fs::remove_file(&lock_path);
                                (None, None)
                            }
                        }
                    }
                    None => {
                        if let Some(state) = super::super::fill::load_file_external_fill_state(
                            self.config.as_ref(),
                            key,
                            share_fingerprint,
                        ) {
                            return FillLockDecision::ReadExternal { state };
                        }
                        return FillLockDecision::WaitExternal { key: key.to_string() };
                    }
                }
            }
        } else {
            (None, None)
        };
        let notify = Arc::new(Notify::new());
        let generation = self.fill_lock_generation.fetch_add(1, Ordering::Relaxed) + 1;
        fill_locks.insert(
            key.to_string(),
            CacheFillLockState {
                notify: notify.clone(),
                acquired_at_unix_ms: now,
                generation,
                share_fingerprint: share_fingerprint.unwrap_or_default().to_string(),
                reader_state: None,
            },
        );
        FillLockDecision::Acquired(CacheFillGuard {
            key: key.to_string(),
            generation,
            fill_locks: Arc::downgrade(&self.fill_locks),
            notify,
            external_lock_path,
            external_state,
        })
    }

    pub(in crate::cache) async fn wait_for_external_fill_lock(
        &self,
        key: &str,
        lock_timeout: std::time::Duration,
        lock_age: std::time::Duration,
    ) -> bool {
        if let Some(store) = self.shared_index_store.as_ref()
            && store.supports_shared_fill_locks()
        {
            return self
                .wait_for_memory_shared_fill_lock(store.as_ref(), key, lock_timeout, lock_age)
                .await;
        }

        let lock_path = shared::shared_fill_lock_path(self.config.as_ref(), key);
        let deadline = tokio::time::Instant::now() + lock_timeout;
        loop {
            match self.file_shared_fill_lock_state(&lock_path, lock_age) {
                FileSharedFillLockState::Missing => return true,
                FileSharedFillLockState::Stale => match std::fs::remove_file(&lock_path) {
                    Ok(()) => return true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return true;
                    }
                    Err(_) => {}
                },
                FileSharedFillLockState::Fresh => {}
            }

            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                return false;
            };
            tokio::time::sleep(remaining.min(std::time::Duration::from_millis(25))).await;
        }
    }

    async fn wait_for_memory_shared_fill_lock(
        &self,
        store: &SharedIndexStore,
        key: &str,
        lock_timeout: std::time::Duration,
        lock_age: std::time::Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + lock_timeout;
        loop {
            let now = unix_time_ms(SystemTime::now());
            match super::super::fill::memory_shared_fill_lock_state(store, key, now, lock_age) {
                Ok(shared::SharedFillLockStatus::Missing) => return true,
                Ok(shared::SharedFillLockStatus::Stale) => {
                    match super::super::fill::clear_stale_memory_shared_fill_lock(
                        store, key, now, lock_age,
                    ) {
                        Ok(true) => return true,
                        Ok(false) => {}
                        Err(_) => {}
                    }
                }
                Ok(shared::SharedFillLockStatus::Fresh) => {}
                Err(_) => return false,
            }

            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                return false;
            };
            tokio::time::sleep(remaining.min(std::time::Duration::from_millis(25))).await;
        }
    }

    fn try_acquire_file_shared_fill_lock(
        &self,
        key: &str,
        lock_age: std::time::Duration,
    ) -> Option<PathBuf> {
        let lock_path = shared::shared_fill_lock_path(self.config.as_ref(), key);
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
                Ok(_) => return Some(lock_path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    match self.file_shared_fill_lock_state(&lock_path, lock_age) {
                        FileSharedFillLockState::Missing | FileSharedFillLockState::Stale => {
                            match std::fs::remove_file(&lock_path) {
                                Ok(()) => continue,
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                    continue;
                                }
                                Err(_) => return None,
                            }
                        }
                        FileSharedFillLockState::Fresh => return None,
                    }
                }
                Err(_) => return None,
            }
        }
    }

    pub(in crate::cache) fn attach_fill_read_state(
        &self,
        key: &str,
        generation: u64,
        state: Arc<CacheFillReadState>,
    ) -> Option<Arc<CacheFillReadState>> {
        let mut fill_locks =
            self.fill_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let lock = fill_locks.get_mut(key)?;
        if lock.generation != generation {
            return None;
        }
        lock.reader_state = Some(state.clone());
        lock.notify.notify_waiters();
        Some(state)
    }

    fn file_shared_fill_lock_state(
        &self,
        lock_path: &Path,
        lock_age: std::time::Duration,
    ) -> FileSharedFillLockState {
        let metadata = match std::fs::metadata(lock_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return FileSharedFillLockState::Missing;
            }
            Err(_) => return FileSharedFillLockState::Fresh,
        };
        let Ok(modified) = metadata.modified() else {
            return FileSharedFillLockState::Fresh;
        };
        if unix_time_ms(SystemTime::now()).saturating_sub(unix_time_ms(modified))
            > lock_age.as_millis() as u64
        {
            FileSharedFillLockState::Stale
        } else {
            FileSharedFillLockState::Fresh
        }
    }
}

impl Drop for CacheFillGuard {
    fn drop(&mut self) {
        if let Some(fill_locks) = self.fill_locks.upgrade() {
            let mut fill_locks = fill_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if fill_locks.get(&self.key).is_some_and(|lock| lock.generation == self.generation) {
                fill_locks.remove(&self.key);
            }
        }
        if let Some(external_state) = &self.external_state {
            external_state.release();
        } else if let Some(external_lock_path) = &self.external_lock_path {
            let _ = std::fs::remove_file(external_lock_path);
        }
        self.notify.notify_waiters();
    }
}
