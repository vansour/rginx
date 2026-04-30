use std::path::Path;

use rginx_core::{Result, ServerTls, VirtualHostTls};

use crate::model::{ServerTlsConfig, VirtualHostTlsConfig};

mod identity;
mod policy;

use identity::{compile_certificate_material, compile_client_auth_policy};
use policy::{
    compile_alpn_protocols, compile_session_cache_size, compile_session_ticket_count,
    compile_tls_cipher_suites, compile_tls_key_exchange_groups, compile_tls_versions,
};

pub(super) fn compile_server_tls(
    tls: Option<ServerTlsConfig>,
    base_dir: &Path,
    allow_missing_identity: bool,
) -> Result<Option<ServerTls>> {
    let Some(ServerTlsConfig {
        cert_path,
        key_path,
        additional_certificates,
        versions,
        cipher_suites,
        key_exchange_groups,
        alpn_protocols,
        ocsp_staple_path,
        ocsp,
        session_resumption,
        session_tickets,
        session_cache_size,
        session_ticket_count,
        client_auth,
    }) = tls
    else {
        return Ok(None);
    };

    let compiled_identity = compile_certificate_material(
        base_dir,
        cert_path,
        key_path,
        additional_certificates,
        ocsp_staple_path,
        ocsp,
        "server TLS",
        allow_missing_identity,
    )?;

    let client_auth = match client_auth {
        Some(client_auth) => Some(compile_client_auth_policy(base_dir, client_auth)?),
        None => None,
    };

    Ok(Some(ServerTls {
        cert_path: compiled_identity.cert_path,
        key_path: compiled_identity.key_path,
        additional_certificates: compiled_identity.additional_certificates,
        versions: compile_tls_versions(versions),
        cipher_suites: compile_tls_cipher_suites(cipher_suites),
        key_exchange_groups: compile_tls_key_exchange_groups(key_exchange_groups),
        alpn_protocols: compile_alpn_protocols(alpn_protocols),
        ocsp_staple_path: compiled_identity.ocsp_staple_path,
        ocsp: compiled_identity.ocsp,
        session_resumption,
        session_tickets,
        session_cache_size: compile_session_cache_size(session_cache_size)?,
        session_ticket_count: compile_session_ticket_count(session_ticket_count)?,
        client_auth,
    }))
}

pub(super) fn compile_virtual_host_tls(
    tls: Option<VirtualHostTlsConfig>,
    base_dir: &Path,
    allow_missing_identity: bool,
) -> Result<Option<VirtualHostTls>> {
    let Some(VirtualHostTlsConfig {
        cert_path,
        key_path,
        additional_certificates,
        ocsp_staple_path,
        ocsp,
        acme: _,
    }) = tls
    else {
        return Ok(None);
    };

    let compiled_identity = compile_certificate_material(
        base_dir,
        cert_path,
        key_path,
        additional_certificates,
        ocsp_staple_path,
        ocsp,
        "vhost TLS",
        allow_missing_identity,
    )?;

    Ok(Some(VirtualHostTls {
        cert_path: compiled_identity.cert_path,
        key_path: compiled_identity.key_path,
        additional_certificates: compiled_identity.additional_certificates,
        ocsp_staple_path: compiled_identity.ocsp_staple_path,
        ocsp: compiled_identity.ocsp,
    }))
}
