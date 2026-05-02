use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::*;

enum SharedFillLockState {
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
    ) -> FillLockDecision {
        let mut fill_locks =
            self.fill_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(lock) = fill_locks.get(key).cloned()
            && now.saturating_sub(lock.acquired_at_unix_ms) <= lock_age.as_millis() as u64
        {
            if let Some(state) = lock.reader_state.filter(|state| state.can_share()) {
                return FillLockDecision::ReadLocal { state };
            }
            return FillLockDecision::WaitLocal { waiter: lock.notify.notified_owned() };
        }

        let external_lock_path = if self.config.shared_index {
            match self.try_acquire_shared_fill_lock(key, lock_age) {
                Some(path) => Some(path),
                None => {
                    if let Some(state) =
                        super::super::fill::load_external_fill_state(self.config.as_ref(), key)
                    {
                        return FillLockDecision::ReadExternal { state };
                    }
                    return FillLockDecision::WaitExternal { key: key.to_string() };
                }
            }
        } else {
            None
        };
        let external_state = external_lock_path.as_ref().and_then(|lock_path| {
            match super::super::fill::create_shared_external_fill_handle(
                self.config.as_ref(),
                key,
                lock_path,
                now,
            ) {
                Ok(state) => Some(state),
                Err(error) => {
                    tracing::warn!(
                        zone = %self.config.name,
                        key = %key,
                        path = %lock_path.display(),
                        %error,
                        "failed to initialize external shared fill state"
                    );
                    let _ = std::fs::remove_file(lock_path);
                    None
                }
            }
        });
        if external_lock_path.is_some() && external_state.is_none() {
            return FillLockDecision::WaitExternal { key: key.to_string() };
        }
        let notify = Arc::new(Notify::new());
        let generation = self.fill_lock_generation.fetch_add(1, Ordering::Relaxed) + 1;
        fill_locks.insert(
            key.to_string(),
            CacheFillLockState {
                notify: notify.clone(),
                acquired_at_unix_ms: now,
                generation,
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
        let lock_path = shared::shared_fill_lock_path(self.config.as_ref(), key);
        let deadline = tokio::time::Instant::now() + lock_timeout;
        loop {
            match self.shared_fill_lock_state(&lock_path, lock_age) {
                SharedFillLockState::Missing => return true,
                SharedFillLockState::Stale => match std::fs::remove_file(&lock_path) {
                    Ok(()) => return true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return true;
                    }
                    Err(_) => {}
                },
                SharedFillLockState::Fresh => {}
            }

            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                return false;
            };
            tokio::time::sleep(remaining.min(std::time::Duration::from_millis(25))).await;
        }
    }

    fn try_acquire_shared_fill_lock(
        &self,
        key: &str,
        lock_age: std::time::Duration,
    ) -> Option<PathBuf> {
        let lock_path = shared::shared_fill_lock_path(self.config.as_ref(), key);
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
                Ok(_) => return Some(lock_path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    match self.shared_fill_lock_state(&lock_path, lock_age) {
                        SharedFillLockState::Missing | SharedFillLockState::Stale => {
                            match std::fs::remove_file(&lock_path) {
                                Ok(()) => continue,
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                    continue;
                                }
                                Err(_) => return None,
                            }
                        }
                        SharedFillLockState::Fresh => return None,
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

    fn shared_fill_lock_state(
        &self,
        lock_path: &Path,
        lock_age: std::time::Duration,
    ) -> SharedFillLockState {
        let metadata = match std::fs::metadata(lock_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return SharedFillLockState::Missing;
            }
            Err(_) => return SharedFillLockState::Fresh,
        };
        let Ok(modified) = metadata.modified() else {
            return SharedFillLockState::Fresh;
        };
        if unix_time_ms(SystemTime::now()).saturating_sub(unix_time_ms(modified))
            > lock_age.as_millis() as u64
        {
            SharedFillLockState::Stale
        } else {
            SharedFillLockState::Fresh
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
        if let Some(external_lock_path) = &self.external_lock_path {
            let _ = std::fs::remove_file(external_lock_path);
        }
        self.notify.notify_waiters();
    }
}
