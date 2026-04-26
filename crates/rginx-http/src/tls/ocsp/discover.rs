use super::*;

pub(crate) fn ocsp_responder_urls_for_certificate(path: &Path) -> Result<Vec<String>> {
    let certs = load_certificate_chain_from_path(path)?;
    let Some(leaf) = certs.first() else {
        return Ok(Vec::new());
    };

    let cert: RasnCertificate = rasn::der::decode(leaf.as_ref()).map_err(|error| {
        Error::Server(format!(
            "failed to parse X.509 certificate `{}` for OCSP responder discovery: {error}",
            path.display()
        ))
    })?;
    Ok(ocsp_responder_urls_from_cert(&cert))
}

fn ocsp_responder_urls_from_cert(cert: &RasnCertificate) -> Vec<String> {
    const OID_AUTHORITY_INFO_ACCESS: &str = "1.3.6.1.5.5.7.1.1";
    const OID_OCSP_ACCESS_METHOD: &str = "1.3.6.1.5.5.7.48.1";

    let Some(extensions) = cert.tbs_certificate.extensions.as_ref() else {
        return Vec::new();
    };

    for extension in extensions.iter() {
        if extension.extn_id.to_string() != OID_AUTHORITY_INFO_ACCESS {
            continue;
        }
        let Ok(aia) = rasn::der::decode::<AuthorityInfoAccessSyntax>(extension.extn_value.as_ref())
        else {
            continue;
        };
        let mut urls = Vec::new();
        for access in aia {
            if access.access_method.to_string() != OID_OCSP_ACCESS_METHOD {
                continue;
            }
            if let RasnGeneralName::Uri(uri) = access.access_location {
                urls.push(uri.to_string());
            }
        }
        if !urls.is_empty() {
            return urls;
        }
    }

    Vec::new()
}
