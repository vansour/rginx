//! Upstream health probing and health-state exports for proxy routing.

use super::*;

mod active_probe;
mod grpc_health_codec;
mod registry;
mod request;

pub use active_probe::probe_upstream_peer;
#[allow(unused_imports)]
pub(crate) use grpc_health_codec::{
    GrpcHealthProbeResult, GrpcHealthServingStatus, decode_grpc_health_check_response,
    encode_grpc_health_check_request, evaluate_grpc_health_probe_response,
};
pub(crate) use registry::{
    ActivePeerBody, ActivePeerGuard, ActiveProbeStatus, PeerFailureStatus, PeerHealthRegistry,
    SelectedPeers,
};
pub use registry::{PeerHealthSnapshot, UpstreamHealthSnapshot};
pub(super) use request::build_active_health_request;
