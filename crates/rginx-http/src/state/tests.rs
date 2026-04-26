use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
};
use rginx_core::{
    Listener, ReturnAction, Route, RouteAccessControl, RouteAction, RouteMatcher, RuntimeSettings,
    Server, Upstream, UpstreamDnsPolicy, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol,
    UpstreamSettings, UpstreamTls, VirtualHost,
};

use super::{
    ConfigSnapshot, ReloadOutcomeSnapshot, SharedState, SnapshotModule, TlsHandshakeFailureReason,
    inspect_certificate, validate_config_transition,
};

mod counters;
mod snapshots;
mod status;
mod support;
mod tls;
mod traffic;
mod transition;
mod upstreams;

pub(crate) use support::*;
