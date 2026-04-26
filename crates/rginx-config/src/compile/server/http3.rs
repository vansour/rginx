use std::path::Path;
use std::time::Duration;

use rginx_core::{Error, ListenerHttp3, Result};

use crate::model::Http3Config;

pub(super) const DEFAULT_HTTP3_MAX_CONCURRENT_STREAMS: usize = 128;
pub(super) const DEFAULT_HTTP3_STREAM_BUFFER_SIZE_BYTES: usize = 64 * 1024;
pub(super) const DEFAULT_HTTP3_ACTIVE_CONNECTION_ID_LIMIT: u32 = 2;
pub(super) const DEFAULT_HTTP3_RETRY: bool = false;
pub(super) const DEFAULT_HTTP3_GSO: bool = false;

pub(super) fn compile_http3(
    http3: Option<Http3Config>,
    tcp_listen_addr: std::net::SocketAddr,
    tls_enabled: bool,
    base_dir: &Path,
) -> Result<Option<ListenerHttp3>> {
    let Some(http3) = http3 else {
        return Ok(None);
    };

    if !tls_enabled {
        return Err(Error::Config(
            "http3 requires tls to be configured on the same listener".to_string(),
        ));
    }

    let listen_addr = match http3.listen {
        Some(listen) => listen.parse()?,
        None => tcp_listen_addr,
    };

    let max_concurrent_streams = compile_http3_usize(
        http3.max_concurrent_streams.unwrap_or(DEFAULT_HTTP3_MAX_CONCURRENT_STREAMS as u64),
        "max_concurrent_streams",
    )?;
    let stream_buffer_size = compile_http3_usize(
        http3.stream_buffer_size_bytes.unwrap_or(DEFAULT_HTTP3_STREAM_BUFFER_SIZE_BYTES as u64),
        "stream_buffer_size_bytes",
    )?;
    let host_key_path = http3.host_key_path.map(|path| super::super::resolve_path(base_dir, path));

    Ok(Some(ListenerHttp3 {
        listen_addr,
        advertise_alt_svc: http3.advertise_alt_svc.unwrap_or(true),
        alt_svc_max_age: Duration::from_secs(http3.alt_svc_max_age_secs.unwrap_or(86_400)),
        max_concurrent_streams,
        stream_buffer_size,
        active_connection_id_limit: http3
            .active_connection_id_limit
            .unwrap_or(DEFAULT_HTTP3_ACTIVE_CONNECTION_ID_LIMIT),
        retry: http3.retry.unwrap_or(DEFAULT_HTTP3_RETRY),
        host_key_path,
        gso: http3.gso.unwrap_or(DEFAULT_HTTP3_GSO),
        early_data_enabled: http3.early_data.unwrap_or(false),
    }))
}

fn compile_http3_usize(raw: u64, field: &str) -> Result<usize> {
    usize::try_from(raw)
        .map_err(|_| Error::Config(format!("server http3 {field} `{raw}` exceeds platform limits")))
}
