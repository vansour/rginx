use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use rginx_core::{Error, Result, ServerTls, VirtualHost};
use rustls::server::ResolvesServerCert;
use rustls::server::ClientHello;
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

/// SNI 证书解析器，支持基于域名选择证书
#[derive(Debug)]
pub struct SniCertificateResolver {
    default: Option<Arc<rustls::sign::CertifiedKey>>,
    by_name: HashMap<String, Arc<rustls::sign::CertifiedKey>>,
}

impl SniCertificateResolver {
    pub fn new(
        default: Option<Arc<rustls::sign::CertifiedKey>>,
        by_name: HashMap<String, Arc<rustls::sign::CertifiedKey>>,
    ) -> Self {
        Self { default, by_name }
    }
}

impl ResolvesServerCert for SniCertificateResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<rustls::sign::CertifiedKey>> {
        if let Some(name) = client_hello.server_name() {
            let name_lower = name.to_lowercase();
            // 先尝试精确匹配
            if let Some(cert) = self.by_name.get(&name_lower) {
                return Some(cert.clone());
            }
            // 尝试通配符匹配
            for (pattern, cert) in &self.by_name {
                if let Some(suffix) = pattern.strip_prefix("*.") {
                    if name_lower.ends_with(&format!(".{suffix}")) || name_lower == suffix {
                        return Some(cert.clone());
                    }
                }
            }
        }
        self.default.clone()
    }
}

/// 构建支持 SNI 的 TLS acceptor
pub fn build_tls_acceptor(
    default_vhost: &VirtualHost,
    vhosts: &[VirtualHost],
) -> Result<Option<TlsAcceptor>> {
    // 收集所有 vhost 的证书
    let mut all_certs: HashMap<String, Arc<rustls::sign::CertifiedKey>> = HashMap::new();
    let mut default_cert: Option<Arc<rustls::sign::CertifiedKey>> = None;

    // 处理 default_vhost
    if let Some(tls) = &default_vhost.tls {
        let cert_key = load_certified_key(tls)?;
        default_cert = Some(cert_key.clone());
        // 为 default_vhost 的 server_names 注册证书
        for name in &default_vhost.server_names {
            all_certs.insert(name.to_lowercase(), cert_key.clone());
        }
    }

    // 处理额外的 vhosts
    for vhost in vhosts {
        if let Some(tls) = &vhost.tls {
            let cert_key = load_certified_key(tls)?;
            for name in &vhost.server_names {
                all_certs.insert(name.to_lowercase(), cert_key.clone());
            }
        }
    }

    // 如果没有默认证书且没有任何 vhost 证书，则不需要 TLS
    if default_cert.is_none() && all_certs.is_empty() {
        return Ok(None);
    }

    let resolver = Arc::new(SniCertificateResolver::new(default_cert, all_certs));

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    Ok(Some(TlsAcceptor::from(Arc::new(config))))
}

fn load_certified_key(tls: &ServerTls) -> Result<Arc<rustls::sign::CertifiedKey>> {
    let certs = load_certificates(tls)?;
    let key = load_private_key(tls)?;

    let certified_key = rustls::sign::CertifiedKey::new(
        certs,
        rustls::crypto::aws_lc_rs::sign::any_supported_type(&key).map_err(|_| {
            Error::Server(format!(
                "server TLS private key file `{}` uses unsupported algorithm",
                tls.key_path.display()
            ))
        })?,
    );

    Ok(Arc::new(certified_key))
}

fn load_certificates(tls: &ServerTls) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let file = File::open(&tls.cert_path)?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Error::Io)?;

    if certs.is_empty() {
        return Err(Error::Server(format!(
            "server TLS certificate file `{}` did not contain any PEM certificates",
            tls.cert_path.display()
        )));
    }

    Ok(certs)
}

fn load_private_key(tls: &ServerTls) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let file = File::open(&tls.key_path)?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader).map_err(Error::Io)?.ok_or_else(|| {
        Error::Server(format!(
            "server TLS private key file `{}` did not contain a supported PEM private key",
            tls.key_path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rginx_core::VirtualHost;

    use super::build_tls_acceptor;

    const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4E\nFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2zn\nyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngq\np7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gp\nqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+\nyfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6\nJrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5\nx23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59\nCiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
    const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQ\nD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";

    #[test]
    fn build_tls_acceptor_returns_none_for_plain_http() {
        let default_vhost = VirtualHost {
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        };
        let vhosts: Vec<VirtualHost> = Vec::new();

        assert!(build_tls_acceptor(&default_vhost, &vhosts).unwrap().is_none());
    }

    #[test]
    fn build_tls_acceptor_loads_valid_pem_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("rginx-server-tls-{unique}"));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
        fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

        let default_vhost = VirtualHost {
            server_names: vec!["localhost".to_string()],
            routes: Vec::new(),
            tls: Some(rginx_core::ServerTls {
                cert_path: cert_path.clone(),
                key_path: key_path.clone(),
            }),
        };
        let vhosts: Vec<VirtualHost> = Vec::new();

        let acceptor = build_tls_acceptor(&default_vhost, &vhosts).expect("TLS acceptor should load");
        assert!(acceptor.is_some());

        fs::remove_file(cert_path).expect("test cert should be removed");
        fs::remove_file(key_path).expect("test key should be removed");
        fs::remove_dir(temp_dir).expect("temp dir should be removed");
    }
}
