use http::Uri;

use super::{normalize_request_path, normalize_request_target};

#[test]
fn normalize_request_path_preserves_asterisk_form() {
    assert_eq!(normalize_request_path("*"), "*");
}

#[test]
fn normalize_request_target_preserves_asterisk_path_and_query_shape() {
    let uri: Uri = "*".parse().expect("asterisk-form URI should parse");
    let normalized = normalize_request_target(&uri);

    assert_eq!(normalized.path, "*");
    assert_eq!(normalized.path_and_query, "*");
}

#[test]
fn normalize_request_path_clamps_above_root_traversal() {
    assert_eq!(normalize_request_path("/a/../../etc/passwd"), "/");
}
