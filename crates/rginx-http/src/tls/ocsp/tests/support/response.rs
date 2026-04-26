use super::*;

pub(crate) fn build_ocsp_response_for_certificate(
    cert_path: &Path,
    issuer: &TestCertifiedKey,
) -> Vec<u8> {
    build_ocsp_response_for_certificate_with_signer(
        cert_path,
        TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
        Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
        TimeOffset::Before(Duration::from_secs(60)),
        RasnCertStatus::Good,
        OcspResponseSigner::Issuer(issuer),
        None,
        false,
        false,
    )
}

pub(crate) fn build_ocsp_response_for_certificate_with_offsets(
    cert_path: &Path,
    issuer: &TestCertifiedKey,
    this_update_offset: TimeOffset,
    next_update_offset: TimeOffset,
) -> Vec<u8> {
    build_ocsp_response_for_certificate_with_signer(
        cert_path,
        this_update_offset,
        Some(next_update_offset),
        this_update_offset,
        RasnCertStatus::Good,
        OcspResponseSigner::Issuer(issuer),
        None,
        false,
        false,
    )
}

pub(crate) fn build_ocsp_response_for_certificate_with_signer(
    cert_path: &Path,
    this_update_offset: TimeOffset,
    next_update_offset: Option<TimeOffset>,
    produced_at_offset: TimeOffset,
    cert_status: RasnCertStatus,
    signer: OcspResponseSigner<'_>,
    response_nonce: Option<&[u8]>,
    duplicate_matching_response: bool,
    tamper_signature: bool,
) -> Vec<u8> {
    let certs = load_certificate_chain_from_path(cert_path).expect("certificate chain should load");
    let cert_id =
        build_rasn_ocsp_cert_id_from_chain(&certs, cert_path).expect("CertId should build");
    let now = SystemTime::now();
    let this_update = ocsp_time_with_offset(now, this_update_offset);
    let produced_at = ocsp_time_with_offset(now, produced_at_offset);
    let next_update = next_update_offset.map(|offset| ocsp_time_with_offset(now, offset));
    let mut responses = vec![RasnSingleResponse {
        cert_id: cert_id.clone(),
        cert_status: cert_status.clone(),
        this_update,
        next_update,
        single_extensions: None,
    }];
    if duplicate_matching_response {
        responses.push(RasnSingleResponse {
            cert_id,
            cert_status,
            this_update,
            next_update,
            single_extensions: None,
        });
    }

    let tbs_response_data = RasnResponseData {
        version: Integer::from(0),
        responder_id: signer.responder_id(),
        produced_at,
        responses,
        response_extensions: response_nonce
            .map(build_ocsp_nonce_extension)
            .transpose()
            .expect("response nonce should encode")
            .map(|extension| vec![extension].into()),
    };
    let tbs_der =
        rasn::der::encode(&tbs_response_data).expect("response data should encode for signing");
    let mut signature = signer.signing_key().sign(&tbs_der).expect("OCSP response should sign");
    if tamper_signature {
        signature[0] ^= 0x55;
    }

    let basic = RasnBasicOcspResponse {
        tbs_response_data,
        signature_algorithm: test_signature_algorithm(signer.signing_key()),
        signature: BitString::from_slice(&signature),
        certs: signer.embedded_certs(),
    };
    let basic_der = rasn::der::encode(&basic).expect("basic OCSP response should encode");
    rasn::der::encode(&RasnOcspResponse {
        status: RasnOcspResponseStatus::Successful,
        bytes: Some(RasnResponseBytes {
            r#type: basic_ocsp_response_type_oid(),
            response: OctetString::from_slice(&basic_der),
        }),
    })
    .expect("OCSP response should encode")
}

pub(crate) fn ocsp_time_with_offset(base: SystemTime, offset: TimeOffset) -> GeneralizedTime {
    let time = match offset {
        TimeOffset::Before(duration) => {
            base.checked_sub(duration).expect("time offset should stay after unix epoch")
        }
        TimeOffset::After(duration) => base + duration,
    };
    generalized_time_from_system_time(time)
}

pub(crate) fn responder_id_for_certificate(cert_der: &[u8]) -> RasnResponderId {
    let cert: RasnCertificate = rasn::der::decode(cert_der).expect("certificate should decode");
    RasnResponderId::ByKey(OctetString::from(
        Sha1::digest(subject_public_key_bytes(&cert)).to_vec(),
    ))
}

pub(crate) fn authority_info_access_extension_value(responder_url: &str) -> Vec<u8> {
    der_sequence([der_sequence([
        vec![0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01],
        der_context_6_ia5_string(responder_url.as_bytes()),
    ])])
}

pub(crate) fn der_sequence<const N: usize>(elements: [Vec<u8>; N]) -> Vec<u8> {
    let payload = elements.into_iter().flatten().collect::<Vec<_>>();
    der_wrap(0x30, payload)
}

pub(crate) fn der_context_6_ia5_string(bytes: &[u8]) -> Vec<u8> {
    der_wrap(0x86, bytes.to_vec())
}

pub(crate) fn der_wrap(tag: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.push(tag);
    encoded.extend(der_length(payload.len()));
    encoded.extend(payload);
    encoded
}

pub(crate) fn der_length(length: usize) -> Vec<u8> {
    if length < 0x80 {
        return vec![length as u8];
    }

    let bytes = length.to_be_bytes().into_iter().skip_while(|byte| *byte == 0).collect::<Vec<_>>();
    let mut encoded = Vec::with_capacity(bytes.len() + 1);
    encoded.push(0x80 | (bytes.len() as u8));
    encoded.extend(bytes);
    encoded
}

pub(crate) fn test_signature_algorithm(key: &KeyPair) -> rasn_pkix::AlgorithmIdentifier {
    let der = if key.algorithm() == &PKCS_ECDSA_P256_SHA256 {
        &[0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02][..]
    } else if key.algorithm() == &PKCS_RSA_SHA256 {
        &[0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b, 0x05, 0x00]
            [..]
    } else if key.algorithm() == &PKCS_ED25519 {
        &[0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70][..]
    } else {
        panic!("unsupported OCSP test signature algorithm");
    };
    rasn::der::decode(der).expect("signature algorithm should decode")
}

pub(crate) fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}

#[derive(Clone, Copy)]
pub(crate) enum TimeOffset {
    Before(Duration),
    After(Duration),
}

pub(crate) enum OcspResponseSigner<'a> {
    Issuer(&'a TestCertifiedKey),
    Delegated(&'a TestCertifiedKey),
}

impl<'a> OcspResponseSigner<'a> {
    pub(crate) fn signing_key(&self) -> &KeyPair {
        match self {
            Self::Issuer(key) | Self::Delegated(key) => &key.signing_key,
        }
    }

    pub(crate) fn responder_id(&self) -> RasnResponderId {
        match self {
            Self::Issuer(key) | Self::Delegated(key) => {
                responder_id_for_certificate(key.cert.der().as_ref())
            }
        }
    }

    pub(crate) fn embedded_certs(&self) -> Option<Vec<rasn_pkix::Certificate>> {
        match self {
            Self::Delegated(key) => Some(vec![
                rasn::der::decode(key.cert.der().as_ref())
                    .expect("delegated responder certificate should decode"),
            ]),
            _ => None,
        }
    }
}
