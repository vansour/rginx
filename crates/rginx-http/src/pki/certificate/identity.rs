use super::ParsedClientIdentity;
use super::decode::decode_certificate;
use super::extensions::subject_alt_dns_names;
use super::helpers::integer_to_serial_string;
use super::name::name_to_string;

pub(crate) fn parse_tls_client_identity<'a>(
    der_chain: impl IntoIterator<Item = &'a [u8]>,
) -> ParsedClientIdentity {
    let mut identity = ParsedClientIdentity {
        subject: None,
        issuer: None,
        serial_number: None,
        san_dns_names: Vec::new(),
        chain_length: 0,
        chain_subjects: Vec::new(),
    };

    for (index, der) in der_chain.into_iter().enumerate() {
        identity.chain_length += 1;
        if let Some(cert) = decode_certificate(der) {
            let extensions = cert.tbs_certificate.extensions.as_ref().map(|value| value.as_slice());
            let subject = name_to_string(&cert.tbs_certificate.subject);
            identity.chain_subjects.push(subject.clone());
            if index == 0 {
                identity.subject = Some(subject);
                identity.issuer = Some(name_to_string(&cert.tbs_certificate.issuer));
                identity.serial_number =
                    Some(integer_to_serial_string(&cert.tbs_certificate.serial_number));
                identity.san_dns_names = subject_alt_dns_names(extensions);
            }
        }
    }

    identity
}
