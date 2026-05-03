use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::cache) struct CacheMetadata {
    #[serde(default)]
    pub(in crate::cache) key: String,
    #[serde(default)]
    pub(in crate::cache) base_key: String,
    #[serde(default)]
    pub(in crate::cache) vary: Vec<CachedVaryHeader>,
    #[serde(default)]
    pub(in crate::cache) tags: Vec<String>,
    pub(in crate::cache) status: u16,
    pub(in crate::cache) headers: Vec<CachedHeader>,
    pub(in crate::cache) stored_at_unix_ms: u64,
    pub(in crate::cache) expires_at_unix_ms: u64,
    #[serde(default)]
    pub(in crate::cache) kind: CacheIndexEntryKind,
    #[serde(default)]
    pub(in crate::cache) grace_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::cache) keep_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::cache) stale_if_error_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::cache) stale_while_revalidate_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::cache) requires_revalidation: bool,
    #[serde(default)]
    pub(in crate::cache) must_revalidate: bool,
    pub(in crate::cache) body_size_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct RawCacheMetadata {
    #[serde(default)]
    key: String,
    #[serde(default)]
    base_key: String,
    #[serde(default)]
    vary: Vec<CachedVaryHeader>,
    #[serde(default)]
    tags: Vec<String>,
    status: u16,
    headers: Vec<CachedHeader>,
    stored_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    #[serde(default)]
    kind: CacheIndexEntryKind,
    #[serde(default)]
    grace_until_unix_ms: Option<u64>,
    #[serde(default)]
    keep_until_unix_ms: Option<u64>,
    #[serde(default)]
    stale_if_error_until_unix_ms: Option<u64>,
    #[serde(default)]
    stale_while_revalidate_until_unix_ms: Option<u64>,
    #[serde(default)]
    requires_revalidation: Option<bool>,
    #[serde(default)]
    must_revalidate: bool,
    body_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::cache) struct CachedHeader {
    pub(in crate::cache) name: String,
    pub(in crate::cache) value: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::cache) struct CachedVaryHeader {
    pub(in crate::cache) name: String,
    #[serde(default)]
    pub(in crate::cache) value: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::cache) struct CacheMetadataInput {
    pub(in crate::cache) kind: CacheIndexEntryKind,
    pub(in crate::cache) base_key: String,
    pub(in crate::cache) vary: Vec<CachedVaryHeaderValue>,
    pub(in crate::cache) tags: Vec<String>,
    pub(in crate::cache) stored_at_unix_ms: u64,
    pub(in crate::cache) expires_at_unix_ms: u64,
    pub(in crate::cache) grace_until_unix_ms: Option<u64>,
    pub(in crate::cache) keep_until_unix_ms: Option<u64>,
    pub(in crate::cache) stale_if_error_until_unix_ms: Option<u64>,
    pub(in crate::cache) stale_while_revalidate_until_unix_ms: Option<u64>,
    pub(in crate::cache) requires_revalidation: bool,
    pub(in crate::cache) must_revalidate: bool,
    pub(in crate::cache) body_size_bytes: usize,
}

pub(in crate::cache) fn cache_metadata(
    key: String,
    status: StatusCode,
    headers: &HeaderMap,
    input: CacheMetadataInput,
) -> CacheMetadata {
    CacheMetadata {
        key,
        base_key: input.base_key,
        vary: input
            .vary
            .into_iter()
            .map(|header| CachedVaryHeader {
                name: header.name.as_str().to_string(),
                value: header.value,
            })
            .collect(),
        tags: input.tags,
        status: status.as_u16(),
        headers: cached_headers(headers, input.body_size_bytes),
        stored_at_unix_ms: input.stored_at_unix_ms,
        expires_at_unix_ms: input.expires_at_unix_ms,
        kind: input.kind,
        grace_until_unix_ms: input.grace_until_unix_ms,
        keep_until_unix_ms: input.keep_until_unix_ms,
        stale_if_error_until_unix_ms: input.stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms: input.stale_while_revalidate_until_unix_ms,
        requires_revalidation: input.requires_revalidation,
        must_revalidate: input.must_revalidate,
        body_size_bytes: input.body_size_bytes,
    }
}

pub(in crate::cache) fn prepare_cached_response_head(
    hash: &str,
    metadata: CacheMetadata,
) -> std::io::Result<PreparedCacheResponseHead> {
    let headers = metadata.headers_map()?;
    let status = StatusCode::from_u16(metadata.status)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let conditional_headers = build_conditional_headers(&headers);
    Ok(PreparedCacheResponseHead {
        hash: hash.to_string(),
        metadata: Arc::new(metadata),
        status,
        headers,
        conditional_headers,
    })
}

pub(in crate::cache) async fn read_cache_metadata(path: &Path) -> std::io::Result<CacheMetadata> {
    let metadata = fs::read(path).await?;
    let raw: RawCacheMetadata = serde_json::from_slice(&metadata)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(CacheMetadata {
        key: raw.key,
        base_key: raw.base_key,
        vary: raw.vary,
        tags: raw.tags,
        status: raw.status,
        headers: raw.headers,
        stored_at_unix_ms: raw.stored_at_unix_ms,
        expires_at_unix_ms: raw.expires_at_unix_ms,
        kind: raw.kind,
        grace_until_unix_ms: raw.grace_until_unix_ms,
        keep_until_unix_ms: raw.keep_until_unix_ms,
        stale_if_error_until_unix_ms: raw.stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms: raw.stale_while_revalidate_until_unix_ms,
        requires_revalidation: raw.requires_revalidation.unwrap_or(raw.must_revalidate),
        must_revalidate: raw.must_revalidate,
        body_size_bytes: raw.body_size_bytes,
    })
}

impl CacheMetadata {
    pub(super) fn headers_map(&self) -> std::io::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        for header in &self.headers {
            let name = HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let value = HeaderValue::from_bytes(&header.value)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            headers.append(name, value);
        }
        Ok(headers)
    }
}
