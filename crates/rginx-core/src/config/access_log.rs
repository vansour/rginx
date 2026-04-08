use std::fmt::Write as _;

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLogFormat {
    template: String,
    segments: Vec<AccessLogSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AccessLogSegment {
    Literal(String),
    Variable(AccessLogVariable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessLogVariable {
    RequestId,
    RemoteAddr,
    PeerAddr,
    Method,
    Host,
    Path,
    Request,
    Status,
    BodyBytesSent,
    ElapsedMs,
    ClientIpSource,
    Vhost,
    Route,
    Scheme,
    HttpVersion,
    TlsVersion,
    TlsAlpn,
    UserAgent,
    Referer,
    TlsClientAuthenticated,
    TlsClientSubject,
    TlsClientSanDnsNames,
    GrpcProtocol,
    GrpcService,
    GrpcMethod,
    GrpcStatus,
    GrpcMessage,
}

#[derive(Debug, Clone, Copy)]
pub struct AccessLogValues<'a> {
    pub request_id: &'a str,
    pub remote_addr: &'a str,
    pub peer_addr: &'a str,
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub request: &'a str,
    pub status: u16,
    pub body_bytes_sent: Option<u64>,
    pub elapsed_ms: u64,
    pub client_ip_source: &'a str,
    pub vhost: &'a str,
    pub route: &'a str,
    pub scheme: &'a str,
    pub http_version: &'a str,
    pub tls_version: Option<&'a str>,
    pub tls_alpn: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub referer: Option<&'a str>,
    pub tls_client_authenticated: bool,
    pub tls_client_subject: Option<&'a str>,
    pub tls_client_san_dns_names: Option<&'a str>,
    pub grpc_protocol: Option<&'a str>,
    pub grpc_service: Option<&'a str>,
    pub grpc_method: Option<&'a str>,
    pub grpc_status: Option<&'a str>,
    pub grpc_message: Option<&'a str>,
}

impl AccessLogFormat {
    pub fn parse(template: impl Into<String>) -> Result<Self> {
        let template = template.into();
        let mut segments = Vec::new();
        let bytes = template.as_bytes();
        let mut literal_start = 0usize;
        let mut index = 0usize;

        while index < bytes.len() {
            if bytes[index] != b'$' {
                index += 1;
                continue;
            }

            if literal_start < index {
                segments
                    .push(AccessLogSegment::Literal(template[literal_start..index].to_string()));
            }

            if let Some(next) = bytes.get(index + 1) {
                if *next == b'$' {
                    segments.push(AccessLogSegment::Literal("$".to_string()));
                    index += 2;
                    literal_start = index;
                    continue;
                }

                if *next == b'{' {
                    let Some(relative_end) = template[index + 2..].find('}') else {
                        return Err(Error::Config(
                            "access_log_format contains an unterminated `${...}` variable"
                                .to_string(),
                        ));
                    };
                    let end = index + 2 + relative_end;
                    let name = &template[index + 2..end];
                    segments.push(AccessLogSegment::Variable(parse_access_log_variable(name)?));
                    index = end + 1;
                    literal_start = index;
                    continue;
                }
            }

            let mut end = index + 1;
            while end < bytes.len() && is_access_log_variable_char(bytes[end]) {
                end += 1;
            }

            if end == index + 1 {
                segments.push(AccessLogSegment::Literal("$".to_string()));
                index += 1;
                literal_start = index;
                continue;
            }

            let name = &template[index + 1..end];
            segments.push(AccessLogSegment::Variable(parse_access_log_variable(name)?));
            index = end;
            literal_start = end;
        }

        if literal_start < template.len() {
            segments.push(AccessLogSegment::Literal(template[literal_start..].to_string()));
        }

        Ok(Self { template, segments })
    }

    pub fn template(&self) -> &str {
        &self.template
    }

    pub fn render(&self, values: &AccessLogValues<'_>) -> String {
        let mut rendered = String::with_capacity(self.template.len() + 64);

        for segment in &self.segments {
            match segment {
                AccessLogSegment::Literal(literal) => rendered.push_str(literal),
                AccessLogSegment::Variable(variable) => match variable {
                    AccessLogVariable::RequestId => rendered.push_str(values.request_id),
                    AccessLogVariable::RemoteAddr => rendered.push_str(values.remote_addr),
                    AccessLogVariable::PeerAddr => rendered.push_str(values.peer_addr),
                    AccessLogVariable::Method => rendered.push_str(values.method),
                    AccessLogVariable::Host => {
                        rendered.push_str(fallback_access_log_value(values.host))
                    }
                    AccessLogVariable::Path => rendered.push_str(values.path),
                    AccessLogVariable::Request => rendered.push_str(values.request),
                    AccessLogVariable::Status => {
                        let _ = write!(rendered, "{}", values.status);
                    }
                    AccessLogVariable::BodyBytesSent => {
                        if let Some(bytes) = values.body_bytes_sent {
                            let _ = write!(rendered, "{bytes}");
                        } else {
                            rendered.push('-');
                        }
                    }
                    AccessLogVariable::ElapsedMs => {
                        let _ = write!(rendered, "{}", values.elapsed_ms);
                    }
                    AccessLogVariable::ClientIpSource => rendered.push_str(values.client_ip_source),
                    AccessLogVariable::Vhost => rendered.push_str(values.vhost),
                    AccessLogVariable::Route => rendered.push_str(values.route),
                    AccessLogVariable::Scheme => rendered.push_str(values.scheme),
                    AccessLogVariable::HttpVersion => rendered.push_str(values.http_version),
                    AccessLogVariable::TlsVersion => {
                        rendered.push_str(fallback_access_log_option(values.tls_version))
                    }
                    AccessLogVariable::TlsAlpn => {
                        rendered.push_str(fallback_access_log_option(values.tls_alpn))
                    }
                    AccessLogVariable::UserAgent => {
                        rendered.push_str(fallback_access_log_option(values.user_agent))
                    }
                    AccessLogVariable::Referer => {
                        rendered.push_str(fallback_access_log_option(values.referer))
                    }
                    AccessLogVariable::TlsClientAuthenticated => rendered
                        .push_str(if values.tls_client_authenticated { "true" } else { "false" }),
                    AccessLogVariable::TlsClientSubject => {
                        rendered.push_str(fallback_access_log_option(values.tls_client_subject))
                    }
                    AccessLogVariable::TlsClientSanDnsNames => rendered
                        .push_str(fallback_access_log_option(values.tls_client_san_dns_names)),
                    AccessLogVariable::GrpcProtocol => {
                        rendered.push_str(fallback_access_log_option(values.grpc_protocol))
                    }
                    AccessLogVariable::GrpcService => {
                        rendered.push_str(fallback_access_log_option(values.grpc_service))
                    }
                    AccessLogVariable::GrpcMethod => {
                        rendered.push_str(fallback_access_log_option(values.grpc_method))
                    }
                    AccessLogVariable::GrpcStatus => {
                        rendered.push_str(fallback_access_log_option(values.grpc_status))
                    }
                    AccessLogVariable::GrpcMessage => {
                        rendered.push_str(fallback_access_log_option(values.grpc_message))
                    }
                },
            }
        }

        rendered
    }
}

fn parse_access_log_variable(name: &str) -> Result<AccessLogVariable> {
    match name {
        "request_id" => Ok(AccessLogVariable::RequestId),
        "remote_addr" | "client_ip" => Ok(AccessLogVariable::RemoteAddr),
        "peer_addr" => Ok(AccessLogVariable::PeerAddr),
        "method" | "request_method" => Ok(AccessLogVariable::Method),
        "host" => Ok(AccessLogVariable::Host),
        "path" | "request_uri" => Ok(AccessLogVariable::Path),
        "request" => Ok(AccessLogVariable::Request),
        "status" => Ok(AccessLogVariable::Status),
        "body_bytes_sent" | "bytes_sent" => Ok(AccessLogVariable::BodyBytesSent),
        "request_time_ms" | "elapsed_ms" => Ok(AccessLogVariable::ElapsedMs),
        "client_ip_source" => Ok(AccessLogVariable::ClientIpSource),
        "vhost" | "server_name" => Ok(AccessLogVariable::Vhost),
        "route" => Ok(AccessLogVariable::Route),
        "scheme" => Ok(AccessLogVariable::Scheme),
        "http_version" | "server_protocol" => Ok(AccessLogVariable::HttpVersion),
        "tls_version" | "ssl_protocol" => Ok(AccessLogVariable::TlsVersion),
        "tls_alpn" => Ok(AccessLogVariable::TlsAlpn),
        "http_user_agent" | "user_agent" => Ok(AccessLogVariable::UserAgent),
        "http_referer" | "referer" => Ok(AccessLogVariable::Referer),
        "tls_client_authenticated" => Ok(AccessLogVariable::TlsClientAuthenticated),
        "tls_client_subject" => Ok(AccessLogVariable::TlsClientSubject),
        "tls_client_san_dns_names" => Ok(AccessLogVariable::TlsClientSanDnsNames),
        "grpc_protocol" => Ok(AccessLogVariable::GrpcProtocol),
        "grpc_service" => Ok(AccessLogVariable::GrpcService),
        "grpc_method" => Ok(AccessLogVariable::GrpcMethod),
        "grpc_status" => Ok(AccessLogVariable::GrpcStatus),
        "grpc_message" => Ok(AccessLogVariable::GrpcMessage),
        _ => Err(Error::Config(format!("access_log_format variable `${name}` is not supported"))),
    }
}

fn is_access_log_variable_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn fallback_access_log_value(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

fn fallback_access_log_option(value: Option<&str>) -> &str {
    value.filter(|value| !value.is_empty()).unwrap_or("-")
}
