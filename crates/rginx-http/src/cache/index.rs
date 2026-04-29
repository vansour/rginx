use super::{CacheIndex, CacheIndexEntry};

impl CacheIndex {
    pub(super) fn insert_entry(
        &mut self,
        key: String,
        entry: CacheIndexEntry,
    ) -> Option<CacheIndexEntry> {
        let existing = self.entries.insert(key, entry.clone());
        if let Some(existing) = &existing {
            self.decrement_hash_ref(&existing.hash);
        }
        self.increment_hash_ref(&entry.hash);
        existing
    }

    pub(super) fn remove_entry(&mut self, key: &str) -> Option<CacheIndexEntry> {
        let removed = self.entries.remove(key)?;
        self.decrement_hash_ref(&removed.hash);
        Some(removed)
    }

    pub(super) fn hash_is_referenced(&self, hash: &str) -> bool {
        self.hash_ref_counts.get(hash).copied().unwrap_or_default() > 0
    }

    fn increment_hash_ref(&mut self, hash: &str) {
        *self.hash_ref_counts.entry(hash.to_string()).or_insert(0) += 1;
    }

    fn decrement_hash_ref(&mut self, hash: &str) {
        let Some(count) = self.hash_ref_counts.get_mut(hash) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.hash_ref_counts.remove(hash);
        }
    }
}

impl CacheIndexEntry {
    pub(super) fn stable_eq(&self, other: &Self) -> bool {
        self.hash == other.hash
            && self.base_key == other.base_key
            && self.vary == other.vary
            && self.body_size_bytes == other.body_size_bytes
            && self.expires_at_unix_ms == other.expires_at_unix_ms
            && self.stale_if_error_until_unix_ms == other.stale_if_error_until_unix_ms
            && self.stale_while_revalidate_until_unix_ms
                == other.stale_while_revalidate_until_unix_ms
            && self.must_revalidate == other.must_revalidate
    }
}
