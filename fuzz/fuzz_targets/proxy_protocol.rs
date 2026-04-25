#![no_main]

use std::net::{Ipv4Addr, SocketAddr};

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (trust_remote_addr, header_bytes) = match data.split_first() {
        Some((flag, rest)) => (flag & 1 == 1, rest),
        None => (false, &[][..]),
    };
    let header = String::from_utf8_lossy(header_bytes);
    let remote_addr = SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 4000));

    let _ = rginx_http::server::parse_proxy_protocol_v1_for_fuzzing(
        &header,
        remote_addr,
        trust_remote_addr,
    );
});
