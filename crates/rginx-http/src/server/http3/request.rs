use super::*;

pub(super) async fn serve_http3_request(
    resolver: RequestResolver<h3_quinn::Connection, Bytes>,
    listener_id: String,
    state: crate::state::SharedState,
    connection_addrs: Arc<ConnectionPeerAddrs>,
) -> Result<()> {
    let (request, stream) = match resolver.resolve_request().await {
        Ok(result) => result,
        Err(error) => {
            state.record_http3_request_resolve_error(&listener_id);
            return Err(Error::Server(format!("failed to resolve http3 request: {error}")));
        }
    };
    let (send_stream, recv_stream) = stream.split();
    let mut request = request.map(|()| {
        crate::handler::boxed_body(super::body::Http3RequestBody::new(
            recv_stream,
            state.clone(),
            listener_id.clone(),
        ))
    });
    *request.version_mut() = Version::HTTP_3;

    let response =
        crate::handler::handle(request, state.clone(), connection_addrs, &listener_id).await;
    super::response::send_http3_response(send_stream, response, &state, &listener_id).await
}
