use super::*;

pub(crate) fn upstream_tls_verify_label(tls: &UpstreamTls) -> &'static str {
    match tls {
        UpstreamTls::NativeRoots => "native_roots",
        UpstreamTls::CustomCa { .. } => "custom_ca",
        UpstreamTls::Insecure => "insecure",
    }
}

pub(crate) fn classify_upstream_tls_failure(error: &impl std::fmt::Display) -> &'static str {
    let error = error.to_string().to_ascii_lowercase();
    if error.contains("unknown ca") || error.contains("unknown issuer") {
        return "unknown_ca";
    }
    if error.contains("revoked") {
        return "certificate_revoked";
    }
    if error.contains("verify_depth") || error.contains("exceeds configured verify_depth") {
        return "verify_depth_exceeded";
    }
    if error.contains("certificate verify failed")
        || error.contains("invalid certificate")
        || error.contains("bad certificate")
        || error.contains("not valid for name")
    {
        return "bad_certificate";
    }
    "-"
}
