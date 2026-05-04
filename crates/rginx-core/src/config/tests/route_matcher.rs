use super::*;

#[test]
fn regex_route_matcher_supports_case_insensitive_path_patterns() {
    let matcher = RouteMatcher::Regex(
        super::super::RouteRegexMatcher::new(
            "^/api/v1/ws/(server|terminal|file)(/.*)?$".to_string(),
            true,
        )
        .expect("regex matcher should compile"),
    );

    assert!(matcher.matches("/api/v1/ws/server"));
    assert!(matcher.matches("/API/V1/WS/terminal/session"));
    assert!(!matcher.matches("/api/v1/ws/metrics"));
    assert!(matcher.priority() > RouteMatcher::Prefix("/".to_string()).priority());
}

#[test]
fn preferred_prefix_route_matcher_preserves_prefix_semantics() {
    let matcher = RouteMatcher::PreferredPrefix("/assets".to_string());

    assert!(matcher.matches("/assets"));
    assert!(matcher.matches("/assets/logo.svg"));
    assert!(!matcher.matches("/assets-v2/logo.svg"));
    assert!(
        matcher.priority()
            > RouteMatcher::Regex(
                super::super::RouteRegexMatcher::new("^/assets/.*$".to_string(), false)
                    .expect("regex matcher should compile"),
            )
            .priority()
    );
}

#[test]
fn regex_route_matcher_uses_equal_priority_for_declaration_order() {
    let broad = RouteMatcher::Regex(
        super::super::RouteRegexMatcher::new("^/api/.*$".to_string(), false)
            .expect("regex matcher should compile"),
    );
    let narrow = RouteMatcher::Regex(
        super::super::RouteRegexMatcher::new("^/api/v1/longer/.*$".to_string(), false)
            .expect("regex matcher should compile"),
    );

    assert_eq!(broad.priority(), narrow.priority());
}

#[test]
fn regex_route_matcher_rejects_oversized_patterns() {
    let pattern = r"\w{100000}".to_string();

    let result = super::super::RouteRegexMatcher::new(pattern, false);
    assert!(result.is_err(), "oversized regex should be rejected");
}
