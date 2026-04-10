use super::*;

pub(super) fn query_admin_socket(
    config_path: &Path,
    request: AdminRequest,
) -> anyhow::Result<AdminResponse> {
    let socket_path = admin_socket_path_for_config(config_path);
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to admin socket {}", socket_path.display()))?;
    serde_json::to_writer(&mut stream, &request)
        .context("failed to encode admin socket request")?;
    stream.write_all(b"\n").context("failed to terminate admin socket request")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to shutdown admin socket write side")?;

    let mut response = String::new();
    BufReader::new(stream)
        .read_to_string(&mut response)
        .context("failed to read admin socket response")?;
    let response: AdminResponse =
        serde_json::from_str(response.trim()).context("failed to decode admin socket response")?;
    match response {
        AdminResponse::Error { message } => Err(anyhow!("admin socket error: {message}")),
        response => Ok(response),
    }
}

pub(super) fn unexpected_admin_response(command: &str, response: &AdminResponse) -> anyhow::Error {
    anyhow!("unexpected admin response for `{command}`: {}", admin_response_kind(response))
}

fn admin_response_kind(response: &AdminResponse) -> &'static str {
    match response {
        AdminResponse::Snapshot(_) => "snapshot",
        AdminResponse::SnapshotVersion(_) => "snapshot_version",
        AdminResponse::Delta(_) => "delta",
        AdminResponse::Status(_) => "status",
        AdminResponse::Counters(_) => "counters",
        AdminResponse::TrafficStats(_) => "traffic_stats",
        AdminResponse::PeerHealth(_) => "peer_health",
        AdminResponse::UpstreamStats(_) => "upstream_stats",
        AdminResponse::Revision(RevisionSnapshot { .. }) => "revision",
        AdminResponse::Error { .. } => "error",
    }
}
