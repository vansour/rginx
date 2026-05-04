use std::error::Error as StdError;

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::body::Body;
use hyper::{Request, Response};

use crate::client_ip::ConnectionPeerAddrs;

pub(crate) type BoxError = Box<dyn StdError + Send + Sync>;
pub(crate) type HttpBody = UnsyncBoxBody<Bytes, BoxError>;
pub(crate) type HttpResponse = Response<HttpBody>;

#[derive(Clone, Copy, Debug)]
pub(crate) struct EarlyDataFlag(pub bool);

mod access_log;
mod dispatch;
mod grpc;
mod response;

pub(crate) use access_log::UpstreamAccessLog;
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
