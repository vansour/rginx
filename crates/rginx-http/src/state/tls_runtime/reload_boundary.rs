use crate::config_transition_boundary;

pub fn tls_reloadable_fields() -> Vec<String> {
    config_transition_boundary().reloadable_fields
}

pub fn tls_restart_required_fields() -> Vec<String> {
    config_transition_boundary().restart_required_fields
}
