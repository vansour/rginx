#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServerNameMatch {
    Exact,
    DotWildcard { suffix_len: usize },
    LeadingWildcard { suffix_len: usize },
    TrailingWildcard { prefix_len: usize },
}

impl ServerNameMatch {
    pub fn priority(self) -> (u8, usize) {
        match self {
            Self::Exact => (4, 0),
            Self::DotWildcard { suffix_len } => (2, suffix_len),
            Self::LeadingWildcard { suffix_len } => (2, suffix_len),
            Self::TrailingWildcard { prefix_len } => (1, prefix_len),
        }
    }
}

pub fn best_matching_server_name_pattern<'a>(
    patterns: impl IntoIterator<Item = &'a str>,
    host: &str,
) -> Option<(&'a str, ServerNameMatch)> {
    patterns
        .into_iter()
        .filter_map(|pattern| match_server_name(pattern, host).map(|matched| (pattern, matched)))
        .max_by(|left, right| {
            left.1.priority().cmp(&right.1.priority()).then_with(|| right.0.cmp(left.0))
        })
}

pub fn match_server_name(pattern: &str, host: &str) -> Option<ServerNameMatch> {
    let hostname = normalize_host_for_match(host);
    let pattern = pattern.trim().to_ascii_lowercase();

    if pattern.is_empty() {
        return None;
    }

    if let Some(suffix) = pattern.strip_prefix('.') {
        if suffix.is_empty() {
            return None;
        }

        if hostname == suffix || hostname.ends_with(&format!(".{suffix}")) {
            return Some(ServerNameMatch::DotWildcard { suffix_len: suffix.len() });
        }

        return None;
    }

    if let Some(suffix) = pattern.strip_prefix("*.") {
        if suffix.is_empty() || hostname == suffix {
            return None;
        }

        return hostname
            .ends_with(&format!(".{suffix}"))
            .then_some(ServerNameMatch::LeadingWildcard { suffix_len: suffix.len() });
    }

    if let Some(prefix) = pattern.strip_suffix(".*") {
        if prefix.is_empty() || hostname == prefix {
            return None;
        }

        return hostname
            .strip_prefix(prefix)
            .filter(|remainder| remainder.starts_with('.') && remainder.len() > 1)
            .map(|_| ServerNameMatch::TrailingWildcard { prefix_len: prefix.len() });
    }

    (hostname == pattern).then_some(ServerNameMatch::Exact)
}

fn normalize_host_for_match(host: &str) -> String {
    if let Some(rest) = host.strip_prefix('[')
        && let Some((addr, _)) = rest.split_once(']')
    {
        return addr.to_ascii_lowercase();
    }

    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}
