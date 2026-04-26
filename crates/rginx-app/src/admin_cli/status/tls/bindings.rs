use crate::admin_cli::render::print_record;

use super::render_string_list;

pub(super) fn print_status_tls_bindings(tls: &rginx_http::TlsRuntimeSnapshot) {
    print_status_tls_vhost_bindings(&tls.vhost_bindings);
    print_status_tls_sni_bindings(&tls.sni_bindings);
    print_status_tls_sni_conflicts(&tls.sni_conflicts);
    print_status_tls_default_certificate_bindings(&tls.default_certificate_bindings);
}

fn print_status_tls_vhost_bindings(bindings: &[rginx_http::TlsVhostBindingSnapshot]) {
    for binding in bindings {
        print_record(
            "status_tls_vhost_binding",
            [
                ("listener", binding.listener_name.clone()),
                ("vhost", binding.vhost_id.clone()),
                ("server_names", render_string_list(&binding.server_names)),
                ("certificate_scopes", render_string_list(&binding.certificate_scopes)),
                ("fingerprints", render_string_list(&binding.fingerprints)),
                ("default_selected", binding.default_selected.to_string()),
            ],
        );
    }
}

fn print_status_tls_sni_bindings(bindings: &[rginx_http::TlsSniBindingSnapshot]) {
    for binding in bindings {
        print_record(
            "status_tls_sni_binding",
            [
                ("listener", binding.listener_name.clone()),
                ("server_name", binding.server_name.clone()),
                ("certificate_scopes", render_string_list(&binding.certificate_scopes)),
                ("fingerprints", render_string_list(&binding.fingerprints)),
                ("default_selected", binding.default_selected.to_string()),
            ],
        );
    }
}

fn print_status_tls_sni_conflicts(bindings: &[rginx_http::TlsSniBindingSnapshot]) {
    for binding in bindings {
        print_record(
            "status_tls_sni_conflict",
            [
                ("listener", binding.listener_name.clone()),
                ("server_name", binding.server_name.clone()),
                ("certificate_scopes", render_string_list(&binding.certificate_scopes)),
                ("fingerprints", render_string_list(&binding.fingerprints)),
            ],
        );
    }
}

fn print_status_tls_default_certificate_bindings(
    bindings: &[rginx_http::TlsDefaultCertificateBindingSnapshot],
) {
    for binding in bindings {
        print_record(
            "status_tls_default_certificate_binding",
            [
                ("listener", binding.listener_name.clone()),
                ("server_name", binding.server_name.clone()),
                ("certificate_scopes", render_string_list(&binding.certificate_scopes)),
                ("fingerprints", render_string_list(&binding.fingerprints)),
            ],
        );
    }
}
