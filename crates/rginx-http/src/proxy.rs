use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use http::header::{
    HeaderMap, HeaderName, HeaderValue, CONNECTION, HOST, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION,
    TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use http::{Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_rustls::{
    ConfigBuilderExt, FixedServerNameResolver, HttpsConnector, HttpsConnectorBuilder,
};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rginx_core::{ConfigSnapshot, Error, ProxyTarget, Upstream, UpstreamPeer, UpstreamTls};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

use crate::handler::HttpResponse;

pub type ProxyClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TlsClientProfile {
    tls: UpstreamTls,
    server_name_override: Option<String>,
}

impl TlsClientProfile {
    fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            tls: upstream.tls.clone(),
            server_name_override: upstream.server_name_override.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ProxyClients {
    clients: Arc<HashMap<TlsClientProfile, ProxyClient>>,
}

impl ProxyClients {
    pub fn from_config(config: &ConfigSnapshot) -> Result<Self, Error> {
        let profiles = config
            .upstreams
            .values()
            .map(|upstream| TlsClientProfile::from_upstream(upstream.as_ref()))
            .collect::<HashSet<_>>();

        let mut clients = HashMap::new();
        for profile in profiles {
            let client = build_client_for_profile(&profile)?;
            clients.insert(profile, client);
        }

        Ok(Self { clients: Arc::new(clients) })
    }

    pub fn for_upstream(&self, upstream: &Upstream) -> Result<ProxyClient, Error> {
        let profile = TlsClientProfile::from_upstream(upstream);
        self.clients.get(&profile).cloned().ok_or_else(|| {
            Error::Server(format!(
                "missing cached proxy client for upstream `{}` with TLS profile {:?}",
                upstream.name, profile
            ))
        })
    }
}

pub async fn forward_request(
    clients: ProxyClients,
    request: Request<Incoming>,
    target: &ProxyTarget,
    remote_addr: SocketAddr,
) -> HttpResponse {
    let upstream = match target.upstream.next_peer() {
        Some(peer) => peer,
        None => {
            tracing::warn!(upstream = %target.upstream_name, "proxy route has no available peers");
            return bad_gateway(format!(
                "upstream `{}` has no configured peers\n",
                target.upstream_name
            ));
        }
    };

    let client = match clients.for_upstream(target.upstream.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                upstream = %target.upstream_name,
                %error,
                "failed to select proxy client"
            );
            return bad_gateway(format!(
                "upstream `{}` TLS client is unavailable\n",
                target.upstream_name
            ));
        }
    };

    let upstream_request = match build_upstream_request(
        request,
        &upstream,
        &target.upstream_name,
        remote_addr,
    )
    .await
    {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(
                upstream = %target.upstream_name,
                peer = %upstream.url,
                %error,
                "failed to build upstream request"
            );
            return bad_gateway(format!(
                "failed to build upstream request for `{}`\n",
                target.upstream_name
            ));
        }
    };

    let upstream_response = match client.request(upstream_request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                upstream = %target.upstream_name,
                peer = %upstream.url,
                %error,
                "upstream request failed"
            );
            return bad_gateway(format!("upstream `{}` is unavailable\n", target.upstream_name));
        }
    };

    match build_downstream_response(upstream_response).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                upstream = %target.upstream_name,
                peer = %upstream.url,
                %error,
                "failed to read upstream response body"
            );
            bad_gateway(format!(
                "failed to read response from upstream `{}`\n",
                target.upstream_name
            ))
        }
    }
}

async fn build_upstream_request(
    request: Request<Incoming>,
    peer: &UpstreamPeer,
    upstream_name: &str,
    remote_addr: SocketAddr,
) -> Result<Request<Full<Bytes>>, Box<dyn std::error::Error + Send + Sync>> {
    let (mut parts, body) = request.into_parts();
    let body_bytes = body.collect().await?.to_bytes();
    let original_host = parts.headers.get(HOST).cloned();

    parts.uri = build_proxy_uri(peer, &parts.uri)?;
    sanitize_request_headers(&mut parts.headers, &peer.authority, original_host, remote_addr)?;

    tracing::debug!(
        upstream = %upstream_name,
        peer = %peer.url,
        uri = %parts.uri,
        "forwarding request to upstream"
    );

    Ok(Request::from_parts(parts, Full::new(body_bytes)))
}

async fn build_downstream_response(
    response: Response<Incoming>,
) -> Result<HttpResponse, hyper::Error> {
    let (parts, body) = response.into_parts();
    let status = parts.status;
    let version = parts.version;
    let body_bytes = body.collect().await?.to_bytes();
    let mut headers = parts.headers;
    sanitize_response_headers(&mut headers);

    let mut downstream = Response::new(Full::new(body_bytes));
    *downstream.status_mut() = status;
    *downstream.version_mut() = version;
    *downstream.headers_mut() = headers;
    Ok(downstream)
}

fn build_client_for_profile(profile: &TlsClientProfile) -> Result<ProxyClient, Error> {
    let mut connector = HttpConnector::new();
    connector.enforce_http(false);

    let tls_config = build_tls_config(&profile.tls)?;
    let builder = HttpsConnectorBuilder::new().with_tls_config(tls_config).https_or_http();
    let builder = if let Some(server_name_override) = &profile.server_name_override {
        let server_name = ServerName::try_from(server_name_override.clone()).map_err(|error| {
            Error::Server(format!(
                "invalid TLS server_name_override `{server_name_override}`: {error}"
            ))
        })?;
        builder.with_server_name_resolver(FixedServerNameResolver::new(server_name))
    } else {
        builder
    };
    let connector = builder.enable_http1().wrap_connector(connector);

    Ok(Client::builder(TokioExecutor::new()).build(connector))
}

fn build_tls_config(tls: &UpstreamTls) -> Result<ClientConfig, Error> {
    match tls {
        UpstreamTls::NativeRoots => {
            let builder = ClientConfig::builder().with_native_roots().map_err(|error| {
                Error::Server(format!("failed to load native TLS roots: {error}"))
            })?;
            Ok(builder.with_no_client_auth())
        }
        UpstreamTls::CustomCa { ca_cert_path } => {
            let roots = load_custom_ca_store(ca_cert_path)?;
            Ok(ClientConfig::builder().with_root_certificates(roots).with_no_client_auth())
        }
        UpstreamTls::Insecure => {
            let verifier = Arc::new(InsecureServerCertVerifier::new());
            Ok(ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth())
        }
    }
}

fn load_custom_ca_store(path: &Path) -> Result<RootCertStore, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs =
        rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>().map_err(|error| {
            Error::Server(format!(
                "failed to parse custom CA certificates from `{}`: {error}",
                path.display()
            ))
        })?;

    let mut roots = RootCertStore::empty();
    if certs.is_empty() {
        let der = std::fs::read(path)?;
        roots.add(CertificateDer::from(der)).map_err(|error| {
            Error::Server(format!(
                "failed to add DER custom CA certificate `{}`: {error}",
                path.display()
            ))
        })?;
        return Ok(roots);
    }

    let (added, _ignored) = roots.add_parsable_certificates(certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server(format!(
            "no valid CA certificates were loaded from `{}`",
            path.display()
        )));
    }

    Ok(roots)
}

fn build_proxy_uri(peer: &UpstreamPeer, original_uri: &Uri) -> Result<Uri, http::Error> {
    let path_and_query = original_uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");

    Uri::builder()
        .scheme(peer.scheme.as_str())
        .authority(peer.authority.as_str())
        .path_and_query(path_and_query)
        .build()
}

fn sanitize_request_headers(
    headers: &mut HeaderMap,
    authority: &str,
    original_host: Option<HeaderValue>,
    remote_addr: SocketAddr,
) -> Result<(), http::header::InvalidHeaderValue> {
    remove_hop_by_hop_headers(headers);
    headers.insert(HOST, HeaderValue::from_str(authority)?);
    headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));

    if let Some(host) = original_host {
        headers.insert("x-forwarded-host", host);
    }

    let forwarded_for = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("{value}, {}", remote_addr.ip()))
        .unwrap_or_else(|| remote_addr.ip().to_string());
    headers.insert("x-forwarded-for", HeaderValue::from_str(&forwarded_for)?);

    Ok(())
}

fn sanitize_response_headers(headers: &mut HeaderMap) {
    remove_hop_by_hop_headers(headers);
}

fn remove_hop_by_hop_headers(headers: &mut HeaderMap) {
    let mut extra_headers = Vec::new();

    for value in headers.get_all(CONNECTION) {
        if let Ok(value) = value.to_str() {
            for item in value.split(',') {
                let trimmed = item.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if let Ok(name) = HeaderName::from_bytes(trimmed.as_bytes()) {
                    extra_headers.push(name);
                }
            }
        }
    }

    for name in extra_headers {
        headers.remove(name);
    }

    for name in [
        CONNECTION,
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        headers.remove(name);
    }

    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}

fn bad_gateway(message: String) -> HttpResponse {
    crate::handler::text_response(StatusCode::BAD_GATEWAY, "text/plain; charset=utf-8", message)
}

#[derive(Debug)]
struct InsecureServerCertVerifier {
    supported_schemes: Vec<SignatureScheme>,
}

impl InsecureServerCertVerifier {
    fn new() -> Self {
        let supported_schemes = rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes();
        Self { supported_schemes }
    }
}

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_schemes.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rginx_core::{Upstream, UpstreamPeer, UpstreamTls};

    use super::{build_proxy_uri, load_custom_ca_store, ProxyClients};

    #[test]
    fn proxy_uri_keeps_path_and_query() {
        let peer = UpstreamPeer {
            url: "http://127.0.0.1:9000".to_string(),
            scheme: "http".to_string(),
            authority: "127.0.0.1:9000".to_string(),
        };

        let uri = build_proxy_uri(&peer, &"/api/demo?x=1".parse().unwrap()).unwrap();
        assert_eq!(uri, "http://127.0.0.1:9000/api/demo?x=1".parse::<http::Uri>().unwrap());
    }

    #[test]
    fn proxy_uri_keeps_https_scheme() {
        let peer = UpstreamPeer {
            url: "https://example.com".to_string(),
            scheme: "https".to_string(),
            authority: "example.com".to_string(),
        };

        let uri = build_proxy_uri(&peer, &"/healthz".parse().unwrap()).unwrap();
        assert_eq!(uri, "https://example.com/healthz".parse::<http::Uri>().unwrap());
    }

    #[test]
    fn load_custom_ca_store_accepts_pem_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let store = load_custom_ca_store(&path).expect("PEM CA should load");
        assert!(!store.is_empty());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    #[test]
    fn proxy_clients_can_select_insecure_and_custom_ca_modes() {
        let insecure = Upstream::new(
            "insecure".to_string(),
            vec![UpstreamPeer {
                url: "https://localhost:9443".to_string(),
                scheme: "https".to_string(),
                authority: "localhost:9443".to_string(),
            }],
            UpstreamTls::Insecure,
            None,
        );

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-select-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let custom = Upstream::new(
            "custom".to_string(),
            vec![UpstreamPeer {
                url: "https://localhost:9443".to_string(),
                scheme: "https".to_string(),
                authority: "localhost:9443".to_string(),
            }],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            None,
        );

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: std::time::Duration::from_secs(1),
            },
            server: rginx_core::Server { listen_addr: "127.0.0.1:8080".parse().unwrap() },
            routes: Vec::new(),
            upstreams: HashMap::from([
                ("insecure".to_string(), Arc::new(insecure)),
                ("custom".to_string(), Arc::new(custom)),
            ]),
        };

        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        assert!(clients.for_upstream(snapshot.upstreams["insecure"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["custom"].as_ref()).is_ok());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    #[test]
    fn proxy_clients_cache_distinguishes_server_name_override() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-override-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let peer = UpstreamPeer {
            url: "https://127.0.0.1:9443".to_string(),
            scheme: "https".to_string(),
            authority: "127.0.0.1:9443".to_string(),
        };
        let first = Upstream::new(
            "first".to_string(),
            vec![peer.clone()],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            Some("api-a.internal".to_string()),
        );
        let second = Upstream::new(
            "second".to_string(),
            vec![peer.clone()],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            Some("api-b.internal".to_string()),
        );
        let duplicate = Upstream::new(
            "duplicate".to_string(),
            vec![peer],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            Some("api-a.internal".to_string()),
        );

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: std::time::Duration::from_secs(1),
            },
            server: rginx_core::Server { listen_addr: "127.0.0.1:8080".parse().unwrap() },
            routes: Vec::new(),
            upstreams: HashMap::from([
                ("first".to_string(), Arc::new(first)),
                ("second".to_string(), Arc::new(second)),
                ("duplicate".to_string(), Arc::new(duplicate)),
            ]),
        };

        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        assert_eq!(clients.clients.len(), 2);
        assert!(clients.for_upstream(snapshot.upstreams["first"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["second"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["duplicate"].as_ref()).is_ok());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    const TEST_CA_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDXTCCAkWgAwIBAgIJAOIvDiVb18eVMA0GCSqGSIb3DQEBCwUAMEUxCzAJBgNV\nBAYTAkFVMRMwEQYDVQQIDApTb21lLVN0YXRlMSEwHwYDVQQKDBhJbnRlcm5ldCBX\naWRnaXRzIFB0eSBMdGQwHhcNMTYwODE0MTY1NjExWhcNMjYwODEyMTY1NjExWjBF\nMQswCQYDVQQGEwJBVTETMBEGA1UECAwKU29tZS1TdGF0ZTEhMB8GA1UECgwYSW50\nZXJuZXQgV2lkZ2l0cyBQdHkgTHRkMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIB\nCgKCAQEArVHWFn52Lbl1l59exduZntVSZyDYpzDND+S2LUcO6fRBWhV/1Kzox+2G\nZptbuMGmfI3iAnb0CFT4uC3kBkQQlXonGATSVyaFTFR+jq/lc0SP+9Bd7SBXieIV\neIXlY1TvlwIvj3Ntw9zX+scTA4SXxH6M0rKv9gTOub2vCMSHeF16X8DQr4XsZuQr\n7Cp7j1I4aqOJyap5JTl5ijmG8cnu0n+8UcRlBzy99dLWJG0AfI3VRJdWpGTNVZ92\naFff3RpK3F/WI2gp3qV1ynRAKuvmncGC3LDvYfcc2dgsc1N6Ffq8GIrkgRob6eBc\nklDHp1d023Lwre+VaVDSo1//Y72UFwIDAQABo1AwTjAdBgNVHQ4EFgQUbNOlA6sN\nXyzJjYqciKeId7g3/ZowHwYDVR0jBBgwFoAUbNOlA6sNXyzJjYqciKeId7g3/Zow\nDAYDVR0TBAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAVVaR5QWLZIRR4Dw6TSBn\nBQiLpBSXN6oAxdDw6n4PtwW6CzydaA+creiK6LfwEsiifUfQe9f+T+TBSpdIYtMv\nZ2H2tjlFX8VrjUFvPrvn5c28CuLI0foBgY8XGSkR2YMYzWw2jPEq3Th/KM5Catn3\nAFm3bGKWMtGPR4v+90chEN0jzaAmJYRrVUh9vea27bOCn31Nse6XXQPmSI6Gyncy\nOAPUsvPClF3IjeL1tmBotWqSGn1cYxLo+Lwjk22A9h6vjcNQRyZF2VLVvtwYrNU3\nmwJ6GCLsLHpwW/yjyvn8iEltnJvByM/eeRnfXV6WDObyiZsE/n6DxIRJodQzFqy9\nGA==\n-----END CERTIFICATE-----\n";
}
