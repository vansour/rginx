use std::collections::HashMap;

mod activate;
mod bind_tcp;
mod bind_udp;
mod drain;
mod group;
mod join;
mod prepare;
mod reconcile;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) use bind_tcp::bind_std_listener;
#[cfg(test)]
pub(super) use bind_udp::{
    bind_std_udp_socket, bind_std_udp_sockets, normalize_inherited_udp_sockets,
};
pub(super) use drain::{abort_listener_worker_groups, initiate_shutdown_for_groups};
pub(super) use group::ListenerWorkerGroup;
pub(super) use join::{join_aborted_listener_worker_groups, join_listener_worker_groups};
pub(super) use prepare::{
    PreparedListenerWorkerGroup, build_initial_listener_groups, prepare_added_listener_bindings,
};
pub(super) use reconcile::{prune_draining_listener_groups, reconcile_listener_worker_groups};

pub(super) type ListenerGroupMap = HashMap<String, ListenerWorkerGroup>;
