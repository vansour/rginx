use super::*;

#[test]
fn validate_config_transition_allows_unchanged_listener() {
    let current = snapshot("127.0.0.1:8080");
    let next = snapshot("127.0.0.1:8080");

    validate_config_transition(&current, &next).expect("transition should allow the same listener");
}

#[test]
fn validate_config_transition_rejects_listener_change() {
    let current = snapshot("127.0.0.1:8080");
    let next = snapshot("127.0.0.1:9090");

    let error = validate_config_transition(&current, &next)
        .expect_err("transition should reject rebinding");
    assert!(error.to_string().contains("reload requires restart"));
    assert!(error.to_string().contains("default.listen"));
}

#[test]
fn validate_config_transition_rejects_worker_thread_change() {
    let mut current = snapshot("127.0.0.1:8080");
    current.runtime.worker_threads = Some(2);
    let mut next = snapshot("127.0.0.1:8080");
    next.runtime.worker_threads = Some(4);

    let error = validate_config_transition(&current, &next)
        .expect_err("transition should reject worker changes");
    assert!(error.to_string().contains("runtime.worker_threads"));
}

#[test]
fn validate_config_transition_rejects_accept_worker_change() {
    let mut current = snapshot("127.0.0.1:8080");
    current.runtime.accept_workers = 1;
    let mut next = snapshot("127.0.0.1:8080");
    next.runtime.accept_workers = 2;

    let error = validate_config_transition(&current, &next)
        .expect_err("transition should reject accept workers");
    assert!(error.to_string().contains("runtime.accept_workers"));
}
