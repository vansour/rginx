use std::fmt::Write as _;

use crate::{Error, Result};

mod helpers;
mod variables;

use helpers::{fallback_access_log_option, fallback_access_log_value, is_access_log_variable_char};
use variables::{AccessLogVariable, parse_access_log_variable};

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
    pub tls_client_issuer: Option<&'a str>,
    pub tls_client_serial: Option<&'a str>,
    pub tls_client_san_dns_names: Option<&'a str>,
    pub tls_client_chain_length: Option<u64>,
    pub tls_client_chain_subjects: Option<&'a str>,
    pub grpc_protocol: Option<&'a str>,
    pub grpc_service: Option<&'a str>,
    pub grpc_method: Option<&'a str>,
    pub grpc_status: Option<&'a str>,
    pub grpc_message: Option<&'a str>,
    pub cache_status: Option<&'a str>,
    pub upstream_name: Option<&'a str>,
    pub upstream_addr: Option<&'a str>,
    pub upstream_status: Option<u16>,
    pub upstream_response_time_ms: Option<u64>,
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
                    AccessLogVariable::TlsClientIssuer => {
                        rendered.push_str(fallback_access_log_option(values.tls_client_issuer))
                    }
                    AccessLogVariable::TlsClientSerial => {
                        rendered.push_str(fallback_access_log_option(values.tls_client_serial))
                    }
                    AccessLogVariable::TlsClientSanDnsNames => rendered
                        .push_str(fallback_access_log_option(values.tls_client_san_dns_names)),
                    AccessLogVariable::TlsClientChainLength => {
                        if let Some(chain_length) = values.tls_client_chain_length {
                            let _ = write!(rendered, "{chain_length}");
                        } else {
                            rendered.push('-');
                        }
                    }
                    AccessLogVariable::TlsClientChainSubjects => rendered
                        .push_str(fallback_access_log_option(values.tls_client_chain_subjects)),
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
                    AccessLogVariable::CacheStatus => {
                        rendered.push_str(fallback_access_log_option(values.cache_status))
                    }
                    AccessLogVariable::UpstreamName => {
                        rendered.push_str(fallback_access_log_option(values.upstream_name))
                    }
                    AccessLogVariable::UpstreamAddr => {
                        rendered.push_str(fallback_access_log_option(values.upstream_addr))
                    }
                    AccessLogVariable::UpstreamStatus => {
                        if let Some(status) = values.upstream_status {
                            let _ = write!(rendered, "{status}");
                        } else {
                            rendered.push('-');
                        }
                    }
                    AccessLogVariable::UpstreamResponseTimeMs => {
                        if let Some(value) = values.upstream_response_time_ms {
                            let _ = write!(rendered, "{value}");
                        } else {
                            rendered.push('-');
                        }
                    }
                },
            }
        }

        rendered
    }
}
