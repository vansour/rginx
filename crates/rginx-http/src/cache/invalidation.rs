use super::{CacheIndex, CacheIndexEntry, CacheInvalidationRule, CacheInvalidationSelector};

pub(in crate::cache) fn invalidation_scope(selector: &CacheInvalidationSelector) -> String {
    match selector {
        CacheInvalidationSelector::All => "all".to_string(),
        CacheInvalidationSelector::Exact(key) => format!("key:{key}"),
        CacheInvalidationSelector::Prefix(prefix) => format!("prefix:{prefix}"),
        CacheInvalidationSelector::Tag(tag) => format!("tag:{tag}"),
    }
}

pub(in crate::cache) fn normalize_cache_tag(tag: &str) -> Option<String> {
    let tag = tag.trim().to_ascii_lowercase();
    (!tag.is_empty()).then_some(tag)
}

pub(in crate::cache) fn entry_is_logically_invalid(
    index: &CacheIndex,
    key: &str,
    entry: &CacheIndexEntry,
) -> bool {
    index.invalidations.iter().any(|rule| invalidation_rule_matches_entry(rule, key, entry))
}

pub(in crate::cache) fn invalidation_rule_matches_entry(
    rule: &CacheInvalidationRule,
    key: &str,
    entry: &CacheIndexEntry,
) -> bool {
    entry.stored_at_unix_ms <= rule.created_at_unix_ms
        && selector_matches_entry(&rule.selector, key, entry)
}

fn selector_matches_entry(
    selector: &CacheInvalidationSelector,
    key: &str,
    entry: &CacheIndexEntry,
) -> bool {
    match selector {
        CacheInvalidationSelector::All => true,
        CacheInvalidationSelector::Exact(expected) => key == expected,
        CacheInvalidationSelector::Prefix(prefix) => key.starts_with(prefix),
        CacheInvalidationSelector::Tag(tag) => entry.tags.iter().any(|candidate| candidate == tag),
    }
}
