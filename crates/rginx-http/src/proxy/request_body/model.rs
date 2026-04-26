use super::*;

pub(in crate::proxy) struct PreparedProxyRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: PreparedRequestBody,
    pub(in crate::proxy) peer_failover_enabled: bool,
    pub(in crate::proxy) wait_for_streaming_body: bool,
}

pub(in crate::proxy) enum PreparedRequestBody {
    Replayable { body: Bytes, trailers: Option<HeaderMap> },
    Streaming(Option<HttpBody>),
}

pub(in crate::proxy) struct BuiltUpstreamRequest {
    pub request: Request<HttpBody>,
    pub body_completion: Option<StreamingBodyCompletion>,
}

pub(in crate::proxy) type StreamingBodyCompletion =
    tokio::sync::oneshot::Receiver<Result<(), BoxError>>;

#[derive(Debug)]
pub(in crate::proxy) enum PrepareRequestError {
    PayloadTooLarge { max_request_body_bytes: usize },
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl PrepareRequestError {
    pub(in crate::proxy) fn payload_too_large(max_request_body_bytes: usize) -> Self {
        Self::PayloadTooLarge { max_request_body_bytes }
    }

    pub(in crate::proxy) fn boxed(error: BoxError) -> Self {
        if let Some(max_request_body_bytes) = request_body_limit_error(error.as_ref()) {
            Self::payload_too_large(max_request_body_bytes)
        } else {
            Self::Other(error)
        }
    }
}

impl std::fmt::Display for PrepareRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge { max_request_body_bytes } => write!(
                formatter,
                "request body exceeded configured limit of {max_request_body_bytes} bytes"
            ),
            Self::Other(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PrepareRequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PayloadTooLarge { .. } => None,
            Self::Other(error) => Some(error.as_ref()),
        }
    }
}
