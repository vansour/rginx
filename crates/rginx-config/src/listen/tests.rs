use super::*;

#[test]
fn parse_listen_addr_accepts_ipv6_with_ports() {
    assert_eq!(
        parse_listen_addr("servers[0].listen[0]", "[::]:8080").unwrap(),
        "[::]:8080".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(
        parse_listen_addr("servers[0].listen[0]", "[::1]:8443").unwrap(),
        "[::1]:8443".parse::<SocketAddr>().unwrap()
    );
}

#[test]
fn parse_listen_addr_rejects_ipv6_without_port() {
    for value in ["[::1]", "::1"] {
        let error = parse_listen_addr("servers[0].listen[0]", value)
            .expect_err("IPv6 listen without port should fail");
        assert!(error.to_string().contains("listen"));
        assert!(error.to_string().contains("invalid"));
    }
}

#[test]
fn parse_listen_addr_normalizes_wildcard_and_port_only_values() {
    assert_eq!(
        parse_listen_addr("servers[0].listen[0]", "*:8080").unwrap(),
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8080))
    );
    assert_eq!(
        parse_listen_addr("servers[0].listen[0]", "8443").unwrap(),
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8443))
    );
}

#[test]
fn parse_vhost_listen_rejects_unsupported_nginx_options() {
    for option in ["default_server", "reuseport"] {
        let error = parse_vhost_listen("servers[0].listen[0]", &format!("127.0.0.1:8080 {option}"))
            .expect_err("unsupported listen option should fail");
        assert!(error.to_string().contains(&format!("listen option `{option}` is not supported")));
    }
}
