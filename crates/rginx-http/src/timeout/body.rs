use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use http::HeaderMap;
use http::header::{HeaderName, HeaderValue};
use hyper::body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use tokio::time::{Instant, Sleep};

use crate::handler::BoxError;

use super::timers::{poll_idle_timer, reset_idle_timer};

mod grpc_deadline;
mod idle;
mod max_bytes;

pub(crate) use grpc_deadline::GrpcDeadlineBody;
pub(crate) use idle::IdleTimeoutBody;
pub(crate) use max_bytes::{MaxBytesBody, RequestBodyLimitError};

fn grpc_deadline_exceeded_trailers(message: &str) -> HeaderMap {
    let mut trailers = HeaderMap::new();
    trailers.insert(HeaderName::from_static("grpc-status"), HeaderValue::from_static("4"));
    if !message.is_empty() {
        trailers.insert(
            HeaderName::from_static("grpc-message"),
            HeaderValue::from_str(message).expect("gRPC timeout message should be a valid header"),
        );
    }
    trailers
}
