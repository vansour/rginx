use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

mod listeners;
mod render;
mod routes;
mod vhosts;

pub(super) fn print_admin_traffic(config_path: &Path, args: &WindowArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetTrafficStats { window_secs: args.window_secs },
    )? {
        AdminResponse::TrafficStats(traffic) => {
            listeners::print_traffic_listeners(&traffic.listeners);
            vhosts::print_traffic_vhosts(&traffic.vhosts);
            routes::print_traffic_routes(&traffic.routes);
            Ok(())
        }
        response => Err(unexpected_admin_response("traffic", &response)),
    }
}
