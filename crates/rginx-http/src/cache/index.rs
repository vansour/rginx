use super::{CacheAccessScheduleEntry, CacheAccessScheduleTicket, CacheIndex, CacheIndexEntry};

impl CacheIndex {
    pub(super) fn insert_entry(
        &mut self,
        key: String,
        entry: CacheIndexEntry,
    ) -> Option<CacheIndexEntry> {
        let scheduled_key = key.clone();
        let existing = self.entries.insert(key, entry.clone());
        if let Some(existing) = &existing {
            self.decrement_hash_ref(&existing.hash);
        }
        self.increment_hash_ref(&entry.hash);
        self.schedule_entry_access(&scheduled_key, entry.last_access_unix_ms);
        existing
    }

    pub(super) fn remove_entry(&mut self, key: &str) -> Option<CacheIndexEntry> {
        let removed = self.entries.remove(key)?;
        self.decrement_hash_ref(&removed.hash);
        self.unschedule_entry_access(key);
        Some(removed)
    }

    pub(super) fn hash_is_referenced(&self, hash: &str) -> bool {
        self.hash_ref_counts.get(hash).copied().unwrap_or_default() > 0
    }

    #[cfg(test)]
    pub(super) fn rebuild_access_schedule(&mut self) {
        self.maintenance_next_ticket = 0;
        self.access_schedule.clear();
        self.access_ticket_by_key.clear();
        let entries = self
            .entries
            .iter()
            .map(|(key, entry)| (key.clone(), entry.last_access_unix_ms))
            .collect::<Vec<_>>();
        for (key, last_access_unix_ms) in entries {
            self.schedule_entry_access(&key, last_access_unix_ms);
        }
    }

    pub(super) fn reschedule_entry_access(&mut self, key: &str, last_access_unix_ms: u64) {
        if self.entries.contains_key(key) {
            self.schedule_entry_access(key, last_access_unix_ms);
        }
    }

    pub(super) fn oldest_scheduled_access_unix_ms(&self) -> Option<u64> {
        self.access_schedule.first().map(|entry| entry.last_access_unix_ms)
    }

    pub(super) fn pop_oldest_scheduled_entry(&mut self) -> Option<(String, u64)> {
        let scheduled = self.access_schedule.pop_first()?;
        self.access_ticket_by_key.remove(&scheduled.key);
        Some((scheduled.key, scheduled.last_access_unix_ms))
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

    fn schedule_entry_access(&mut self, key: &str, last_access_unix_ms: u64) {
        self.unschedule_entry_access(key);
        let ticket = self.maintenance_next_ticket;
        self.maintenance_next_ticket = self.maintenance_next_ticket.saturating_add(1);
        self.access_schedule.insert(CacheAccessScheduleEntry {
            last_access_unix_ms,
            ticket,
            key: key.to_string(),
        });
        self.access_ticket_by_key
            .insert(key.to_string(), CacheAccessScheduleTicket { last_access_unix_ms, ticket });
    }

    fn unschedule_entry_access(&mut self, key: &str) {
        let Some(ticket) = self.access_ticket_by_key.remove(key) else {
            return;
        };
        self.access_schedule.remove(&CacheAccessScheduleEntry {
            last_access_unix_ms: ticket.last_access_unix_ms,
            ticket: ticket.ticket,
            key: key.to_string(),
        });
    }
}

impl CacheIndexEntry {
    pub(super) fn is_hit_for_pass(&self) -> bool {
        matches!(self.kind, super::CacheIndexEntryKind::HitForPass)
    }

    pub(super) fn stable_eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.stored_at_unix_ms == other.stored_at_unix_ms
            && self.grace_until_unix_ms == other.grace_until_unix_ms
            && self.keep_until_unix_ms == other.keep_until_unix_ms
            && self.hash == other.hash
            && self.base_key == other.base_key
            && self.vary == other.vary
            && self.tags == other.tags
            && self.body_size_bytes == other.body_size_bytes
            && self.expires_at_unix_ms == other.expires_at_unix_ms
            && self.stale_if_error_until_unix_ms == other.stale_if_error_until_unix_ms
            && self.stale_while_revalidate_until_unix_ms
                == other.stale_while_revalidate_until_unix_ms
            && self.requires_revalidation == other.requires_revalidation
            && self.must_revalidate == other.must_revalidate
    }
}
