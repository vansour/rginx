use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rustls_pemfile::certs;

use super::connection::parse_tls_client_identity;
use super::proxy_protocol::parse_proxy_protocol_v1;

#[test]
fn proxy_protocol_v1_parses_tcp4_source_address() {
    let source = parse_proxy_protocol_v1(
        "PROXY TCP4 198.51.100.9 203.0.113.10 12345 443\r\n",
        "10.0.0.1:4000".parse().unwrap(),
        true,
    )
    .expect("header should parse");

    assert_eq!(source, Some("198.51.100.9:12345".parse().unwrap()));
}

#[test]
fn proxy_protocol_v1_accepts_unknown_transport() {
    let source =
        parse_proxy_protocol_v1("PROXY UNKNOWN\r\n", "10.0.0.1:4000".parse().unwrap(), true)
            .expect("unknown header should parse");

    assert_eq!(source, None);
}

#[test]
fn proxy_protocol_v1_rejects_invalid_headers() {
    let error = parse_proxy_protocol_v1("BROKEN\r\n", "10.0.0.1:4000".parse().unwrap(), true)
        .expect_err("invalid header should fail");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn parse_tls_client_identity_extracts_subject_and_dns_san() {
    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .expect("certificate params should build");
    params.distinguished_name.push(DnType::CommonName, "localhost");
    let key_pair = KeyPair::generate().expect("keypair should generate");
    let cert = params.self_signed(&key_pair).expect("cert should generate");
    let pem = cert.pem();
    let mut reader = std::io::Cursor::new(pem.as_bytes());
    let cert = certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate PEM should parse")
        .remove(0);

    let identity = parse_tls_client_identity(std::iter::once(cert.as_ref()));

    assert!(identity.subject.as_deref().is_some_and(|subject| subject.contains("CN=localhost")));
    assert!(identity.issuer.as_deref().is_some_and(|issuer| issuer.contains("CN=localhost")));
    assert!(identity.serial_number.is_some());
    assert!(identity.san_dns_names.iter().any(|san| san == "localhost"));
    assert_eq!(identity.chain_length, 1);
    assert_eq!(identity.chain_subjects.len(), 1);
}

#[test]
fn parse_tls_client_identity_preserves_leaf_fields_and_chain_order() {
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "rginx test root");
    let ca_key = KeyPair::generate().expect("CA keypair should generate");
    let _ca_cert = ca_params.self_signed(&ca_key).expect("CA cert should self-sign");
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

    let mut leaf_params = CertificateParams::new(vec!["client.example.com".to_string()])
        .expect("leaf params should build");
    leaf_params.distinguished_name.push(DnType::CommonName, "client.example.com");
    let leaf_key = KeyPair::generate().expect("leaf keypair should generate");
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_issuer)
        .expect("leaf cert should be signed by CA");

    let pem = format!("{}{}", leaf_cert.pem(), _ca_cert.pem());
    let mut reader = std::io::Cursor::new(pem.as_bytes());
    let certs = certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate PEM chain should parse");

    let identity = parse_tls_client_identity(certs.iter().map(|cert| cert.as_ref()));

    assert_eq!(identity.chain_length, 2);
    assert_eq!(identity.san_dns_names, vec!["client.example.com".to_string()]);
    assert!(identity.subject.as_deref().is_some_and(|subject| subject.contains("CN=client.example.com")));
    assert!(identity.issuer.as_deref().is_some_and(|issuer| issuer.contains("CN=rginx test root")));
    assert_eq!(identity.chain_subjects.len(), 2);
    assert!(identity.chain_subjects[0].contains("CN=client.example.com"));
    assert!(identity.chain_subjects[1].contains("CN=rginx test root"));
}
