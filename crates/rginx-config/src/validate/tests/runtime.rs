use super::*;

#[test]
fn validate_rejects_zero_runtime_worker_threads() {
    let mut config = base_config();
    config.runtime.worker_threads = Some(0);

    let error = validate(&config).expect_err("zero runtime worker threads should be rejected");
    assert!(error.to_string().contains("runtime.worker_threads must be greater than 0"));
}

#[test]
fn validate_rejects_zero_runtime_accept_workers() {
    let mut config = base_config();
    config.runtime.accept_workers = Some(0);

    let error = validate(&config).expect_err("zero runtime accept workers should be rejected");
    assert!(error.to_string().contains("runtime.accept_workers must be greater than 0"));
}
