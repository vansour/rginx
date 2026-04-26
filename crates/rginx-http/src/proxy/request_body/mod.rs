use std::error::Error as StdError;

use super::grpc_web::{GrpcWebMode, GrpcWebRequestBody, GrpcWebTextDecodeBody};
use super::*;
use crate::handler::boxed_body;

mod limits;
mod model;
mod prepare;
mod replay;
mod streaming;
#[cfg(test)]
mod tests;

pub(super) use limits::request_body_limit_error;
pub(super) use model::{
    BuiltUpstreamRequest, PrepareRequestError, PreparedProxyRequest, PreparedRequestBody,
    StreamingBodyCompletion,
};
pub(super) use replay::can_retry_peer_request;
#[cfg(test)]
pub(super) use replay::is_idempotent_method;
