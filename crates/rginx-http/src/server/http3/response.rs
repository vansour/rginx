use super::*;

pub(super) async fn send_http3_response<S>(
    mut stream: RequestStream<S, Bytes>,
    response: HttpResponse,
    state: &crate::state::SharedState,
    listener_id: &str,
) -> Result<()>
where
    S: SendStream<Bytes>,
{
    let (parts, mut body) = response.into_parts();
    let mut response_builder = Response::builder().status(parts.status);
    for (name, value) in &parts.headers {
        response_builder = response_builder.header(name, value);
    }
    let response = response_builder.body(()).map_err(|error| {
        Error::Server(format!("failed to build http3 response headers: {error}"))
    })?;
    stream.send_response(response).await.map_err(|error| {
        state.record_http3_response_stream_error(listener_id);
        Error::Server(format!("failed to send http3 response headers: {error}"))
    })?;

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| {
            Error::Server(format!("failed to read response body frame for http3: {error}"))
        })?;
        match frame.into_data() {
            Ok(data) => {
                if !data.is_empty() {
                    tracing::debug!(len = data.len(), "http3 sending response data frame");
                    stream.send_data(data).await.map_err(|error| {
                        state.record_http3_response_stream_error(listener_id);
                        Error::Server(format!("failed to send http3 response body: {error}"))
                    })?;
                }
            }
            Err(frame) => {
                if let Ok(trailers) = frame.into_trailers() {
                    tracing::debug!(count = trailers.len(), "http3 sending response trailers");
                    stream.send_trailers(trailers).await.map_err(|error| {
                        state.record_http3_response_stream_error(listener_id);
                        Error::Server(format!("failed to send http3 response trailers: {error}"))
                    })?;
                }
            }
        }
    }

    stream.finish().await.map_err(|error| {
        state.record_http3_response_stream_error(listener_id);
        Error::Server(format!("failed to finish http3 response stream: {error}"))
    })
}
