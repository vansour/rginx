use std::error::Error as StdError;
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::{Bytes, BytesMut};
use http::header::{
    CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap, HeaderName, HeaderValue, REFERER, USER_AGENT,
};
use http::{Method, StatusCode, Uri, Version};
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::body::{Body, Frame, SizeHint};
use hyper::{Request, Response};
use rginx_core::{
    AccessLogFormat, AccessLogValues, ConfigSnapshot, Route, RouteAction, VirtualHost,
};

use crate::client_ip::{ClientAddress, ConnectionPeerAddrs, resolve_client_address};
use crate::router;
use crate::state::{ActiveState, SharedState};

pub(crate) type BoxError = Box<dyn StdError + Send + Sync>;
pub(crate) type HttpBody = UnsyncBoxBody<Bytes, BoxError>;
pub(crate) type HttpResponse = Response<HttpBody>;

#[derive(Clone, Copy, Debug)]
pub(crate) struct EarlyDataFlag(pub bool);

mod access_log;
mod dispatch;
mod grpc;
mod response;

pub use dispatch::handle;
pub(crate) use grpc::{GrpcStatusCode, grpc_error_response};
pub(crate) use response::{full_body, text_response};

pub(crate) fn boxed_body<B>(body: B) -> HttpBody
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError> + 'static,
{
    body.map_err(Into::into).boxed_unsync()
}

pub(crate) fn attach_connection_metadata<B>(
    request: &mut Request<B>,
    connection: &ConnectionPeerAddrs,
) {
    request.extensions_mut().insert(EarlyDataFlag(connection.early_data));
    if let Some(identity) = connection.tls_client_identity.clone() {
        request.extensions_mut().insert(identity);
    }
}

#[cfg(test)]
mod tests;
