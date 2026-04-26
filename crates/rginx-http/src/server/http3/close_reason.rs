pub(super) fn is_clean_http3_accept_close(error: &h3::error::ConnectionError) -> bool {
    if error.is_h3_no_error() {
        return true;
    }

    let message = error.to_string();
    matches!(
        message.as_str(),
        "Remote error: ApplicationClose: 0x0"
            | "Remote error: ApplicationClose: 0"
            | "Remote error: Error undefined by h3: closed by peer: ApplicationClose: 0x0"
            | "Remote error: Error undefined by h3: closed by peer: ApplicationClose: 0"
    )
}
