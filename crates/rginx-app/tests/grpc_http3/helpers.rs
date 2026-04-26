use super::*;

pub(super) fn root_store_from_pem(cert_pem: &str) -> Result<RootCertStore, String> {
    let cert = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse certificate PEM: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "certificate PEM did not contain a certificate".to_string())?;
    let mut roots = RootCertStore::empty();
    roots.add(cert).map_err(|error| format!("failed to add root certificate: {error}"))?;
    Ok(roots)
}

pub(super) fn load_certs(path: &Path) -> Vec<CertificateDer<'static>> {
    CertificateDer::pem_file_iter(path)
        .expect("certificate file should open")
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate PEM should parse")
}

pub(super) fn load_private_key(path: &Path) -> PrivateKeyDer<'static> {
    PrivateKeyDer::from_pem_file(path).expect("private key PEM should parse")
}

pub(super) fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}

pub(super) fn generate_cert(hostname: &str) -> rcgen::CertifiedKey<rcgen::KeyPair> {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}

pub(super) fn decode_grpc_web_response(body: &[u8]) -> (Vec<Bytes>, HeaderMap) {
    let mut frames = Vec::new();
    let mut trailers = HeaderMap::new();
    let mut cursor = body;

    while cursor.len() >= 5 {
        let flags = cursor[0];
        let len = u32::from_be_bytes([cursor[1], cursor[2], cursor[3], cursor[4]]) as usize;
        let frame_len = 5 + len;
        let frame = &cursor[..frame_len];
        let payload = &frame[5..];
        if flags & 0x80 == 0 {
            frames.push(Bytes::copy_from_slice(payload));
        } else {
            for line in payload.split(|byte| *byte == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.is_empty() {
                    continue;
                }
                let separator = line
                    .iter()
                    .position(|byte| *byte == b':')
                    .expect("trailer line should contain ':'");
                let (name, value) = line.split_at(separator);
                trailers.append(
                    hyper::http::header::HeaderName::from_bytes(name)
                        .expect("trailer name should parse"),
                    HeaderValue::from_bytes(value[1..].trim_ascii())
                        .expect("trailer value should parse"),
                );
            }
        }
        cursor = &cursor[frame_len..];
    }

    (frames, trailers)
}
