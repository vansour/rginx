use super::dispatch::http_version_label;
use super::grpc::GrpcObservability;
use super::*;
use crate::client_ip::TlsClientIdentity;

pub(super) struct AccessLogContext<'a> {
    pub(crate) request_id: &'a str,
    pub(crate) method: &'a str,
    pub(crate) host: &'a str,
    pub(crate) path: &'a str,
    pub(crate) request_version: Version,
    pub(crate) user_agent: Option<&'a str>,
    pub(crate) referer: Option<&'a str>,
    pub(crate) client_address: &'a ClientAddress,
    pub(crate) vhost: &'a str,
    pub(crate) route: &'a str,
    pub(crate) status: u16,
    pub(crate) elapsed_ms: u64,
    pub(crate) downstream_scheme: &'a str,
    pub(crate) tls_version: Option<&'a str>,
    pub(crate) tls_alpn: Option<&'a str>,
    pub(crate) body_bytes_sent: Option<u64>,
    pub(crate) tls_client_identity: Option<&'a TlsClientIdentity>,
    pub(crate) grpc: Option<&'a GrpcObservability>,
}

#[derive(Debug, Clone)]
pub(super) struct OwnedAccessLogContext {
    pub(crate) request_id: String,
    pub(crate) method: String,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) request_version: Version,
    pub(crate) user_agent: Option<String>,
    pub(crate) referer: Option<String>,
    pub(crate) client_address: ClientAddress,
    pub(crate) vhost: String,
    pub(crate) route: String,
    pub(crate) status: u16,
    pub(crate) elapsed_ms: u64,
    pub(crate) downstream_scheme: String,
    pub(crate) tls_version: Option<String>,
    pub(crate) tls_alpn: Option<String>,
    pub(crate) body_bytes_sent: Option<u64>,
    pub(crate) tls_client_identity: Option<TlsClientIdentity>,
}

impl OwnedAccessLogContext {
    pub(super) fn as_borrowed<'a>(
        &'a self,
        grpc: Option<&'a GrpcObservability>,
    ) -> AccessLogContext<'a> {
        AccessLogContext {
            request_id: &self.request_id,
            method: &self.method,
            host: &self.host,
            path: &self.path,
            request_version: self.request_version,
            user_agent: self.user_agent.as_deref(),
            referer: self.referer.as_deref(),
            client_address: &self.client_address,
            vhost: &self.vhost,
            route: &self.route,
            status: self.status,
            elapsed_ms: self.elapsed_ms,
            downstream_scheme: &self.downstream_scheme,
            tls_version: self.tls_version.as_deref(),
            tls_alpn: self.tls_alpn.as_deref(),
            body_bytes_sent: self.body_bytes_sent,
            tls_client_identity: self.tls_client_identity.as_ref(),
            grpc,
        }
    }
}

pub(super) fn log_access_event(format: Option<&AccessLogFormat>, context: AccessLogContext<'_>) {
    if let Some(format) = format {
        let line = render_access_log_line(format, &context);
        tracing::info!(target: "rginx_http::access", "{line}");
        return;
    }

    tracing::info!(
        request_id = context.request_id,
        method = context.method,
        host = context.host,
        path = context.path,
        client_ip = %context.client_address.client_ip,
        client_ip_source = context.client_address.source.as_str(),
        peer_addr = %context.client_address.peer_addr,
        vhost = context.vhost,
        route = context.route,
        status = context.status,
        grpc_protocol = context.grpc.map_or("-", |grpc| grpc.protocol.as_str()),
        grpc_service = context.grpc.map_or("-", |grpc| grpc.service.as_str()),
        grpc_method = context.grpc.map_or("-", |grpc| grpc.method.as_str()),
        grpc_status = context
            .grpc
            .and_then(|grpc| grpc.status.as_deref())
            .unwrap_or("-"),
        grpc_message = context
            .grpc
            .and_then(|grpc| grpc.message.as_deref())
            .unwrap_or("-"),
        tls_version = context.tls_version.unwrap_or("-"),
        tls_alpn = context.tls_alpn.unwrap_or("-"),
        tls_client_authenticated = context.tls_client_identity.is_some(),
        tls_client_subject = context
            .tls_client_identity
            .and_then(|identity| identity.subject.as_deref())
            .unwrap_or("-"),
        tls_client_issuer = context
            .tls_client_identity
            .and_then(|identity| identity.issuer.as_deref())
            .unwrap_or("-"),
        tls_client_serial = context
            .tls_client_identity
            .and_then(|identity| identity.serial_number.as_deref())
            .unwrap_or("-"),
        tls_client_san_dns_names = joined_tls_client_san_dns_names(context.tls_client_identity)
            .as_deref()
            .unwrap_or("-"),
        tls_client_chain_length = context
            .tls_client_identity
            .map(|identity| identity.chain_length)
            .unwrap_or(0),
        tls_client_chain_subjects = joined_tls_client_chain_subjects(context.tls_client_identity)
            .as_deref()
            .unwrap_or("-"),
        elapsed_ms = context.elapsed_ms,
        "http access"
    );
}

pub(super) fn render_access_log_line(
    format: &AccessLogFormat,
    context: &AccessLogContext<'_>,
) -> String {
    let request = format!(
        "{} {} {}",
        context.method,
        context.path,
        http_version_label(context.request_version)
    );
    let remote_addr = context.client_address.client_ip.to_string();
    let peer_addr = context.client_address.peer_addr.to_string();
    let tls_client_san_dns_names = joined_tls_client_san_dns_names(context.tls_client_identity);
    let tls_client_chain_subjects = joined_tls_client_chain_subjects(context.tls_client_identity);
    format.render(&AccessLogValues {
        request_id: context.request_id,
        remote_addr: &remote_addr,
        peer_addr: &peer_addr,
        method: context.method,
        host: context.host,
        path: context.path,
        request: &request,
        status: context.status,
        body_bytes_sent: context.body_bytes_sent,
        elapsed_ms: context.elapsed_ms,
        client_ip_source: context.client_address.source.as_str(),
        vhost: context.vhost,
        route: context.route,
        scheme: context.downstream_scheme,
        http_version: http_version_label(context.request_version),
        tls_version: context.tls_version,
        tls_alpn: context.tls_alpn,
        user_agent: context.user_agent,
        referer: context.referer,
        tls_client_authenticated: context.tls_client_identity.is_some(),
        tls_client_subject: context
            .tls_client_identity
            .and_then(|identity| identity.subject.as_deref()),
        tls_client_issuer: context
            .tls_client_identity
            .and_then(|identity| identity.issuer.as_deref()),
        tls_client_serial: context
            .tls_client_identity
            .and_then(|identity| identity.serial_number.as_deref()),
        tls_client_san_dns_names: tls_client_san_dns_names.as_deref(),
        tls_client_chain_length: context
            .tls_client_identity
            .map(|identity| identity.chain_length as u64),
        tls_client_chain_subjects: tls_client_chain_subjects.as_deref(),
        grpc_protocol: context.grpc.map(|grpc| grpc.protocol.as_str()),
        grpc_service: context.grpc.map(|grpc| grpc.service.as_str()),
        grpc_method: context.grpc.map(|grpc| grpc.method.as_str()),
        grpc_status: context.grpc.and_then(|grpc| grpc.status.as_deref()),
        grpc_message: context.grpc.and_then(|grpc| grpc.message.as_deref()),
    })
}

fn joined_tls_client_san_dns_names(identity: Option<&TlsClientIdentity>) -> Option<String> {
    let names = identity
        .map(|identity| {
            identity
                .san_dns_names
                .iter()
                .filter(|value| !value.is_empty())
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (!names.is_empty()).then(|| names.join(","))
}

fn joined_tls_client_chain_subjects(identity: Option<&TlsClientIdentity>) -> Option<String> {
    let subjects = identity
        .map(|identity| {
            identity
                .chain_subjects
                .iter()
                .filter(|value| !value.is_empty())
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (!subjects.is_empty()).then(|| subjects.join(","))
}
