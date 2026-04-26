use super::*;

const HTTP3_ACTIVE_CONNECTION_ID_LIMIT_NO_MIGRATION: u32 = 2;
const HTTP3_ACTIVE_CONNECTION_ID_LIMIT_MIGRATION: u32 = 5;

pub fn bind_http3_endpoint(
    listener: &rginx_core::Listener,
    default_vhost: &rginx_core::VirtualHost,
    vhosts: &[rginx_core::VirtualHost],
) -> Result<Option<quinn::Endpoint>> {
    let listen_addr = match listener.http3.as_ref() {
        Some(http3) => http3.listen_addr,
        None => return Ok(None),
    };
    let socket = std::net::UdpSocket::bind(listen_addr).map_err(Error::Io)?;
    socket.set_nonblocking(true).map_err(Error::Io)?;
    bind_http3_endpoint_with_socket(listener, default_vhost, vhosts, socket).map(Some)
}

pub fn bind_http3_endpoint_with_socket(
    listener: &rginx_core::Listener,
    default_vhost: &rginx_core::VirtualHost,
    vhosts: &[rginx_core::VirtualHost],
    socket: std::net::UdpSocket,
) -> Result<quinn::Endpoint> {
    let rustls_server_config = build_http3_server_config(
        listener.server.tls.as_ref(),
        listener.server.default_certificate.as_deref(),
        listener.tls_enabled(),
        default_vhost,
        vhosts,
        listener.http3.as_ref().is_some_and(|http3| http3.early_data_enabled),
    )?
    .ok_or_else(|| {
        Error::Config("http3 listener requires downstream TLS termination".to_string())
    })?;

    let host_key_material =
        super::host_key::load_or_create_http3_host_key(listener.http3.as_ref())?;
    let endpoint_config =
        build_http3_endpoint_config(listener.http3.as_ref(), host_key_material.as_deref())?;
    let mut quic_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(rustls_server_config).map_err(
            |error| {
                Error::Server(format!(
                    "failed to build quic server config for http3 listener: {error}"
                ))
            },
        )?,
    ));
    apply_http3_server_runtime(
        listener.http3.as_ref(),
        host_key_material.as_deref(),
        &mut quic_config,
    )?;
    let runtime = quinn::default_runtime()
        .ok_or_else(|| Error::Server("no async runtime found for http3 endpoint".to_string()))?;
    quinn::Endpoint::new(endpoint_config, Some(quic_config), socket, runtime).map_err(Error::Io)
}

fn apply_http3_server_runtime(
    http3: Option<&rginx_core::ListenerHttp3>,
    host_key_material: Option<&[u8]>,
    quic_config: &mut quinn::ServerConfig,
) -> Result<()> {
    let Some(http3) = http3 else {
        return Ok(());
    };

    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(
        quinn::VarInt::try_from(http3.max_concurrent_streams as u64).map_err(|_| {
            Error::Config(format!(
                "http3 max_concurrent_streams `{}` exceeds QUIC transport limits",
                http3.max_concurrent_streams
            ))
        })?,
    );
    transport.stream_receive_window(
        quinn::VarInt::try_from(http3.stream_buffer_size as u64).map_err(|_| {
            Error::Config(format!(
                "http3 stream_buffer_size `{}` exceeds QUIC transport limits",
                http3.stream_buffer_size
            ))
        })?,
    );
    let receive_window = (http3.max_concurrent_streams as u128)
        .checked_mul(http3.stream_buffer_size as u128)
        .ok_or_else(|| {
            Error::Config(format!(
                "http3 receive window derived from max_concurrent_streams={} and stream_buffer_size={} exceeds platform limits",
                http3.max_concurrent_streams, http3.stream_buffer_size
            ))
        })?;
    let receive_window = u64::try_from(receive_window).map_err(|_| {
        Error::Config(format!(
            "http3 receive window derived from max_concurrent_streams={} and stream_buffer_size={} exceeds platform limits",
            http3.max_concurrent_streams, http3.stream_buffer_size
        ))
    })?;
    transport.receive_window(quinn::VarInt::try_from(receive_window).map_err(|_| {
        Error::Config(format!(
            "http3 receive window derived from max_concurrent_streams={} and stream_buffer_size={} exceeds QUIC transport limits",
            http3.max_concurrent_streams, http3.stream_buffer_size
        ))
    })?);
    transport.enable_segmentation_offload(http3.gso);
    quic_config.transport_config(Arc::new(transport));
    quic_config.migration(
        http3.active_connection_id_limit != HTTP3_ACTIVE_CONNECTION_ID_LIMIT_NO_MIGRATION,
    );

    if let Some(host_key_material) = host_key_material {
        quic_config.token_key(Arc::new(hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(
            &super::host_key::derive_labeled_key_material(
                host_key_material,
                b"rginx-http3-token-key",
            ),
        )));
    }

    Ok(())
}

fn build_http3_endpoint_config(
    http3: Option<&rginx_core::ListenerHttp3>,
    host_key_material: Option<&[u8]>,
) -> Result<quinn::EndpointConfig> {
    let mut endpoint_config = quinn::EndpointConfig::default();
    let Some(http3) = http3 else {
        return Ok(endpoint_config);
    };

    if let Some(host_key_material) = host_key_material {
        endpoint_config.reset_key(Arc::new(hmac::Key::new(
            hmac::HMAC_SHA256,
            &super::host_key::derive_labeled_key_material(
                host_key_material,
                b"rginx-http3-reset-key",
            ),
        )));
    }

    match http3.active_connection_id_limit {
        HTTP3_ACTIVE_CONNECTION_ID_LIMIT_NO_MIGRATION => {
            endpoint_config
                .cid_generator(|| Box::new(quinn_proto::RandomConnectionIdGenerator::new(0)));
        }
        HTTP3_ACTIVE_CONNECTION_ID_LIMIT_MIGRATION => {
            let host_key_material = host_key_material.ok_or_else(|| {
                Error::Config(
                    "http3 active_connection_id_limit `5` requires host_key_path to be configured"
                        .to_string(),
                )
            })?;
            let key = super::host_key::derive_hashed_connection_id_key(host_key_material);
            endpoint_config.cid_generator(move || {
                Box::new(quinn_proto::HashedConnectionIdGenerator::from_key(key))
            });
        }
        unsupported => {
            return Err(Error::Config(format!(
                "http3 active_connection_id_limit `{unsupported}` is not supported by the current QUIC stack"
            )));
        }
    }

    Ok(endpoint_config)
}
