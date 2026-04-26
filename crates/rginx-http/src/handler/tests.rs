use std::collections::HashMap;
use std::time::Duration;

use base64::Engine as _;
use bytes::BytesMut;
use http::{HeaderMap, HeaderValue, Request, StatusCode, header::HOST};
use http_body_util::BodyExt;
use rginx_core::{
    AccessLogFormat, ConfigSnapshot, GrpcRouteMatch, ReturnAction, Route, RouteAccessControl,
    RouteAction, RouteMatcher, RuntimeSettings, Server, VirtualHost, default_server_header,
};

use super::access_log::{AccessLogContext, render_access_log_line};
use super::dispatch::{
    authorize_route, finalize_downstream_response, response_body_bytes_sent,
    select_route_for_request,
};
use super::grpc::{
    GrpcObservability, GrpcWebObservabilityParser, decode_grpc_web_text_observability_final,
    grpc_observability, grpc_request_metadata,
};
use super::{GrpcStatusCode, attach_connection_metadata, grpc_error_response, text_response};
use crate::client_ip::{ClientAddress, ClientIpSource, ConnectionPeerAddrs, TlsClientIdentity};
use crate::compression::ResponseCompressionOptions;

mod observability;
mod responses;
mod routing;
mod support;

pub(crate) use support::*;
