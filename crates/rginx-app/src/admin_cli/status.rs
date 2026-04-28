use super::cache::print_status_cache;
use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

mod listeners;
mod runtime;
mod tls;
mod upstream_tls;

pub(super) fn print_admin_status(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetStatus)? {
        AdminResponse::Status(status) => {
            runtime::print_status_summary(&status);
            listeners::print_status_listeners(&status.listeners);
            tls::print_status_tls(&status.tls);
            upstream_tls::print_status_upstream_tls(&status.upstream_tls);
            print_status_cache(&status.cache);
            Ok(())
        }
        response => Err(unexpected_admin_response("status", &response)),
    }
}
