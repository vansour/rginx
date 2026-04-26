mod bindings;
mod certificates;
mod listeners;
mod ocsp;

use crate::admin_cli::render::render_optional_string_list;

pub(super) fn print_status_tls(tls: &rginx_http::TlsRuntimeSnapshot) {
    listeners::print_status_tls_listeners(&tls.listeners);
    certificates::print_status_tls_certificates(&tls.certificates);
    ocsp::print_status_tls_ocsp(&tls.ocsp);
    bindings::print_status_tls_bindings(tls);
}

fn render_optional_value<T: ToString>(value: Option<T>) -> String {
    value.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string())
}

fn render_optional_path(value: Option<&std::path::Path>) -> String {
    value.map(|path| path.display().to_string()).unwrap_or_else(|| "-".to_string())
}

fn render_string_list(values: &[String]) -> String {
    render_optional_string_list(Some(values))
}
