use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use ipnet::IpNet;
use rginx_core::{
    AccessLogFormat, Error, Listener, Result, Server, ServerCertificateBundle,
    ServerClientAuthMode, ServerClientAuthPolicy, ServerTls, TlsCipherSuite, TlsKeyExchangeGroup,
    TlsVersion, VirtualHostTls,
};

use crate::model::{
    ListenerConfig, ServerCertificateBundleConfig, ServerClientAuthModeConfig, ServerConfig,
    ServerTlsConfig, TlsCipherSuiteConfig, TlsKeyExchangeGroupConfig, TlsVersionConfig,
    VirtualHostTlsConfig,
};

pub(super) struct CompiledServer {
    pub listener: Listener,
    pub server_names: Vec<String>,
}

pub(super) fn compile_legacy_server(
    server: ServerConfig,
    base_dir: &Path,
    any_vhost_tls: bool,
) -> Result<CompiledServer> {
    let ServerConfig {
        listen,
        proxy_protocol,
        default_certificate,
        server_names,
        trusted_proxies,
        keep_alive,
        max_headers,
        max_request_body_bytes,
        max_connections,
        header_read_timeout_secs,
        request_body_read_timeout_secs,
        response_write_timeout_secs,
        access_log_format,
        tls,
    } = server;

    let listen = listen.expect("legacy server listen should be validated before compile");
    let compiled = compile_server_fields(
        ServerFieldConfig {
            listen,
            default_certificate,
            trusted_proxies,
            keep_alive,
            max_headers,
            max_request_body_bytes,
            max_connections,
            header_read_timeout_secs,
            request_body_read_timeout_secs,
            response_write_timeout_secs,
            access_log_format,
            tls,
        },
        base_dir,
    )?;

    Ok(CompiledServer {
        listener: Listener {
            id: "default".to_string(),
            name: "default".to_string(),
            server: compiled.server,
            tls_termination_enabled: compiled.server_tls.is_some() || any_vhost_tls,
            proxy_protocol_enabled: proxy_protocol.unwrap_or(false),
        },
        server_names,
    })
}

pub(super) fn compile_listeners(
    listeners: Vec<ListenerConfig>,
    base_dir: &Path,
) -> Result<Vec<Listener>> {
    listeners
        .into_iter()
        .map(|listener| {
            let ListenerConfig {
                name,
                listen,
                proxy_protocol,
                default_certificate,
                trusted_proxies,
                keep_alive,
                max_headers,
                max_request_body_bytes,
                max_connections,
                header_read_timeout_secs,
                request_body_read_timeout_secs,
                response_write_timeout_secs,
                access_log_format,
                tls,
            } = listener;

            let compiled = compile_server_fields(
                ServerFieldConfig {
                    listen,
                    default_certificate,
                    trusted_proxies,
                    keep_alive,
                    max_headers,
                    max_request_body_bytes,
                    max_connections,
                    header_read_timeout_secs,
                    request_body_read_timeout_secs,
                    response_write_timeout_secs,
                    access_log_format,
                    tls,
                },
                base_dir,
            )?;

            Ok(Listener {
                id: explicit_listener_id(&name),
                name,
                server: compiled.server,
                tls_termination_enabled: compiled.server_tls.is_some(),
                proxy_protocol_enabled: proxy_protocol.unwrap_or(false),
            })
        })
        .collect()
}

fn explicit_listener_id(name: &str) -> String {
    format!("listener:{}", name.trim().to_ascii_lowercase())
}

pub(super) fn compile_server_tls(
    tls: Option<ServerTlsConfig>,
    base_dir: &Path,
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
        "server TLS",
    )?;

    let client_auth = match client_auth {
        Some(client_auth) => {
            let ca_cert_path = super::resolve_path(base_dir, client_auth.ca_cert_path);
            if !ca_cert_path.is_file() {
                return Err(Error::Config(format!(
                    "server TLS client auth CA file `{}` does not exist or is not a file",
                    ca_cert_path.display()
                )));
            }

            let crl_path = match client_auth.crl_path {
                Some(path) => {
                    let resolved = super::resolve_path(base_dir, path);
                    if !resolved.is_file() {
                        return Err(Error::Config(format!(
                            "server TLS client auth CRL file `{}` does not exist or is not a file",
                            resolved.display()
                        )));
                    }
                    Some(resolved)
                }
                None => None,
            };

            Some(ServerClientAuthPolicy {
                mode: match client_auth.mode {
                    ServerClientAuthModeConfig::Optional => ServerClientAuthMode::Optional,
                    ServerClientAuthModeConfig::Required => ServerClientAuthMode::Required,
                },
                ca_cert_path,
                verify_depth: client_auth.verify_depth,
                crl_path,
            })
        }
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
) -> Result<Option<VirtualHostTls>> {
    let Some(VirtualHostTlsConfig {
        cert_path,
        key_path,
        additional_certificates,
        ocsp_staple_path,
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
        "vhost TLS",
    )?;

    Ok(Some(VirtualHostTls {
        cert_path: compiled_identity.cert_path,
        key_path: compiled_identity.key_path,
        additional_certificates: compiled_identity.additional_certificates,
        ocsp_staple_path: compiled_identity.ocsp_staple_path,
    }))
}

struct CompiledServerFields {
    server: Server,
    server_tls: Option<ServerTls>,
}

struct ServerFieldConfig {
    listen: String,
    default_certificate: Option<String>,
    trusted_proxies: Vec<String>,
    keep_alive: Option<bool>,
    max_headers: Option<u64>,
    max_request_body_bytes: Option<u64>,
    max_connections: Option<u64>,
    header_read_timeout_secs: Option<u64>,
    request_body_read_timeout_secs: Option<u64>,
    response_write_timeout_secs: Option<u64>,
    access_log_format: Option<String>,
    tls: Option<ServerTlsConfig>,
}

fn compile_server_fields(
    config: ServerFieldConfig,
    base_dir: &Path,
) -> Result<CompiledServerFields> {
    let ServerFieldConfig {
        listen,
        default_certificate,
        trusted_proxies,
        keep_alive,
        max_headers,
        max_request_body_bytes,
        max_connections,
        header_read_timeout_secs,
        request_body_read_timeout_secs,
        response_write_timeout_secs,
        access_log_format,
        tls,
    } = config;

    let server_tls = compile_server_tls(tls, base_dir)?;
    Ok(CompiledServerFields {
        server: Server {
            listen_addr: listen.parse()?,
            default_certificate: compile_default_certificate(default_certificate),
            trusted_proxies: compile_trusted_proxies(trusted_proxies)?,
            keep_alive: keep_alive.unwrap_or(true),
            max_headers: compile_max_headers(max_headers)?,
            max_request_body_bytes: compile_max_request_body_bytes(max_request_body_bytes)?,
            max_connections: compile_max_connections(max_connections)?,
            header_read_timeout: header_read_timeout_secs.map(Duration::from_secs),
            request_body_read_timeout: request_body_read_timeout_secs.map(Duration::from_secs),
            response_write_timeout: response_write_timeout_secs.map(Duration::from_secs),
            access_log_format: compile_access_log_format(access_log_format)?,
            tls: server_tls.clone(),
        },
        server_tls,
    })
}

fn compile_default_certificate(default_certificate: Option<String>) -> Option<String> {
    default_certificate.map(|name| name.trim().to_lowercase())
}

fn compile_max_headers(max_headers: Option<u64>) -> Result<Option<usize>> {
    max_headers
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!("server max_headers `{limit}` exceeds platform limits"))
            })
        })
        .transpose()
}

fn compile_max_request_body_bytes(max_request_body_bytes: Option<u64>) -> Result<Option<usize>> {
    max_request_body_bytes
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!(
                    "server max_request_body_bytes `{limit}` exceeds platform limits"
                ))
            })
        })
        .transpose()
}

fn compile_max_connections(max_connections: Option<u64>) -> Result<Option<usize>> {
    max_connections
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!("server max_connections `{limit}` exceeds platform limits"))
            })
        })
        .transpose()
}

fn compile_access_log_format(access_log_format: Option<String>) -> Result<Option<AccessLogFormat>> {
    access_log_format.map(AccessLogFormat::parse).transpose()
}

fn compile_tls_versions(versions: Option<Vec<TlsVersionConfig>>) -> Option<Vec<TlsVersion>> {
    versions.map(|versions| {
        versions
            .into_iter()
            .map(|version| match version {
                TlsVersionConfig::Tls12 => TlsVersion::Tls12,
                TlsVersionConfig::Tls13 => TlsVersion::Tls13,
            })
            .collect()
    })
}

fn compile_tls_cipher_suites(
    cipher_suites: Option<Vec<TlsCipherSuiteConfig>>,
) -> Option<Vec<TlsCipherSuite>> {
    cipher_suites.map(|cipher_suites| {
        cipher_suites
            .into_iter()
            .map(|suite| match suite {
                TlsCipherSuiteConfig::Tls13Aes256GcmSha384 => TlsCipherSuite::Tls13Aes256GcmSha384,
                TlsCipherSuiteConfig::Tls13Aes128GcmSha256 => TlsCipherSuite::Tls13Aes128GcmSha256,
                TlsCipherSuiteConfig::Tls13Chacha20Poly1305Sha256 => {
                    TlsCipherSuite::Tls13Chacha20Poly1305Sha256
                }
                TlsCipherSuiteConfig::TlsEcdheEcdsaWithAes256GcmSha384 => {
                    TlsCipherSuite::TlsEcdheEcdsaWithAes256GcmSha384
                }
                TlsCipherSuiteConfig::TlsEcdheEcdsaWithAes128GcmSha256 => {
                    TlsCipherSuite::TlsEcdheEcdsaWithAes128GcmSha256
                }
                TlsCipherSuiteConfig::TlsEcdheEcdsaWithChacha20Poly1305Sha256 => {
                    TlsCipherSuite::TlsEcdheEcdsaWithChacha20Poly1305Sha256
                }
                TlsCipherSuiteConfig::TlsEcdheRsaWithAes256GcmSha384 => {
                    TlsCipherSuite::TlsEcdheRsaWithAes256GcmSha384
                }
                TlsCipherSuiteConfig::TlsEcdheRsaWithAes128GcmSha256 => {
                    TlsCipherSuite::TlsEcdheRsaWithAes128GcmSha256
                }
                TlsCipherSuiteConfig::TlsEcdheRsaWithChacha20Poly1305Sha256 => {
                    TlsCipherSuite::TlsEcdheRsaWithChacha20Poly1305Sha256
                }
            })
            .collect()
    })
}

fn compile_tls_key_exchange_groups(
    groups: Option<Vec<TlsKeyExchangeGroupConfig>>,
) -> Option<Vec<TlsKeyExchangeGroup>> {
    groups.map(|groups| {
        groups
            .into_iter()
            .map(|group| match group {
                TlsKeyExchangeGroupConfig::X25519 => TlsKeyExchangeGroup::X25519,
                TlsKeyExchangeGroupConfig::Secp256r1 => TlsKeyExchangeGroup::Secp256r1,
                TlsKeyExchangeGroupConfig::Secp384r1 => TlsKeyExchangeGroup::Secp384r1,
                TlsKeyExchangeGroupConfig::X25519Mlkem768 => TlsKeyExchangeGroup::X25519Mlkem768,
                TlsKeyExchangeGroupConfig::Secp256r1Mlkem768 => {
                    TlsKeyExchangeGroup::Secp256r1Mlkem768
                }
                TlsKeyExchangeGroupConfig::Mlkem768 => TlsKeyExchangeGroup::Mlkem768,
                TlsKeyExchangeGroupConfig::Mlkem1024 => TlsKeyExchangeGroup::Mlkem1024,
            })
            .collect()
    })
}

fn compile_alpn_protocols(alpn_protocols: Option<Vec<String>>) -> Option<Vec<String>> {
    alpn_protocols.map(|protocols| {
        protocols.into_iter().map(|protocol| protocol.trim().to_string()).collect()
    })
}

fn compile_session_cache_size(session_cache_size: Option<u64>) -> Result<Option<usize>> {
    session_cache_size
        .map(|size| {
            usize::try_from(size).map_err(|_| {
                Error::Config(format!(
                    "server TLS session_cache_size `{size}` exceeds platform limits"
                ))
            })
        })
        .transpose()
}

fn compile_session_ticket_count(session_ticket_count: Option<u64>) -> Result<Option<usize>> {
    session_ticket_count
        .map(|count| {
            usize::try_from(count).map_err(|_| {
                Error::Config(format!(
                    "server TLS session_ticket_count `{count}` exceeds platform limits"
                ))
            })
        })
        .transpose()
}

struct CompiledCertificateMaterial {
    cert_path: std::path::PathBuf,
    key_path: std::path::PathBuf,
    additional_certificates: Vec<ServerCertificateBundle>,
    ocsp_staple_path: Option<std::path::PathBuf>,
}

fn compile_certificate_material(
    base_dir: &Path,
    cert_path: String,
    key_path: String,
    additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
    ocsp_staple_path: Option<String>,
    label: &str,
) -> Result<CompiledCertificateMaterial> {
    let cert_path = super::resolve_path(base_dir, cert_path);
    if !cert_path.is_file() {
        return Err(Error::Config(format!(
            "{label} certificate file `{}` does not exist or is not a file",
            cert_path.display()
        )));
    }

    let key_path = super::resolve_path(base_dir, key_path);
    if !key_path.is_file() {
        return Err(Error::Config(format!(
            "{label} private key file `{}` does not exist or is not a file",
            key_path.display()
        )));
    }

    let ocsp_staple_path = compile_ocsp_staple_path(base_dir, ocsp_staple_path, label)?;
    let additional_certificates = additional_certificates
        .unwrap_or_default()
        .into_iter()
        .map(|bundle| {
            compile_certificate_bundle(base_dir, bundle, &format!("{label} additional certificate"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(CompiledCertificateMaterial {
        cert_path,
        key_path,
        additional_certificates,
        ocsp_staple_path,
    })
}

fn compile_certificate_bundle(
    base_dir: &Path,
    bundle: ServerCertificateBundleConfig,
    label: &str,
) -> Result<ServerCertificateBundle> {
    let cert_path = super::resolve_path(base_dir, bundle.cert_path);
    if !cert_path.is_file() {
        return Err(Error::Config(format!(
            "{label} file `{}` does not exist or is not a file",
            cert_path.display()
        )));
    }

    let key_path = super::resolve_path(base_dir, bundle.key_path);
    if !key_path.is_file() {
        return Err(Error::Config(format!(
            "{label} private key file `{}` does not exist or is not a file",
            key_path.display()
        )));
    }

    let ocsp_staple_path = compile_ocsp_staple_path(base_dir, bundle.ocsp_staple_path, label)?;

    Ok(ServerCertificateBundle { cert_path, key_path, ocsp_staple_path })
}

fn compile_ocsp_staple_path(
    base_dir: &Path,
    path: Option<String>,
    label: &str,
) -> Result<Option<std::path::PathBuf>> {
    match path {
        Some(path) => {
            let resolved = super::resolve_path(base_dir, path);
            if !resolved.is_file() {
                return Err(Error::Config(format!(
                    "{label} OCSP staple file `{}` does not exist or is not a file",
                    resolved.display()
                )));
            }
            Ok(Some(resolved))
        }
        None => Ok(None),
    }
}

fn compile_trusted_proxies(values: Vec<String>) -> Result<Vec<IpNet>> {
    values
        .into_iter()
        .map(|value| {
            let normalized = normalize_trusted_proxy(&value).ok_or_else(|| {
                Error::Config(format!(
                    "server trusted_proxies entry `{value}` must be a valid IP address or CIDR"
                ))
            })?;

            normalized.parse::<IpNet>().map_err(|error| {
                Error::Config(format!("server trusted_proxies entry `{value}` is invalid: {error}"))
            })
        })
        .collect()
}

fn normalize_trusted_proxy(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains('/') {
        return Some(trimmed.to_string());
    }

    let ip = trimmed.parse::<IpAddr>().ok()?;
    Some(match ip {
        IpAddr::V4(_) => format!("{trimmed}/32"),
        IpAddr::V6(_) => format!("{trimmed}/128"),
    })
}
