use super::*;

#[test]
fn wildcard_sni_selection_prefers_more_specific_patterns() {
    let certs = vec![Arc::new(dummy_certified_key())];
    let by_name = HashMap::from([
        ("*.example.com".to_string(), certs.clone()),
        ("*.api.example.com".to_string(), certs.clone()),
    ]);

    let selected = best_matching_wildcard_certificates(&by_name, "edge.api.example.com")
        .expect("more specific wildcard should match");
    assert_eq!(selected.len(), 1);
    assert!(best_matching_wildcard_certificates(&by_name, "example.com").is_none());
}
