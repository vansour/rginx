use dioxus::prelude::*;

use rginx_control_types::{
    AuthLoginRequest, AuthRole, AuthenticatedActor, DashboardSummary, DnsRuntimeQueryStat,
    DnsRuntimeStatus, NodeDetailResponse, NodeSummary,
};
use serde::Deserialize;
use web_sys::EventSource;

use crate::api::{self, ApiError, EventStream};
use crate::display::{
    StreamState, format_bool, format_list, format_optional, format_unix_ms, pretty_json,
    stream_state_label,
};
use crate::runtime::{
    HttpCountersSnapshot, RuntimeStatusSnapshot, TlsRuntimeSnapshot, parse_counters, parse_runtime,
    parse_traffic, parse_upstream_health, parse_upstream_stats,
};
use crate::{Route, SessionContext, reset_session, use_session};

mod components;
mod dashboard;
mod forms;
mod login;
mod nodes;
mod not_found;
mod shared;
mod streams;

use components::*;
use forms::LoginForm;
use shared::*;
use streams::*;

pub(crate) use dashboard::Dashboard;
pub(crate) use login::Login;
pub(crate) use nodes::{EdgeNodes, NodeDetail, NodeTls};
pub(crate) use not_found::NotFound;
