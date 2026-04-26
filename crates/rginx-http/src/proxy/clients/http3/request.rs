use http::{Request, Response, Version};
use http_body_util::BodyExt;
use rginx_core::{Error, Upstream};

use crate::handler::HttpBody;
use crate::proxy::ResolvedUpstreamPeer;

use super::Http3Client;
use super::connect::server_name_for_peer;
use super::response_body::{response_size_hint, streaming_response_body};
use super::session::Http3SessionKey;

impl Http3Client {
    pub(in super::super) async fn request(
        &self,
        upstream: &Upstream,
        peer: &ResolvedUpstreamPeer,
        request: Request<HttpBody>,
    ) -> Result<Response<HttpBody>, Error> {
        let server_name = server_name_for_peer(upstream, peer)?;
        let session = self
            .session_for(
                Http3SessionKey { remote_addr: peer.socket_addr, server_name },
                &peer.display_url,
            )
            .await?;
        let mut send_request = session.sender().await;

        let (parts, mut body) = request.into_parts();
        let mut request_builder =
            Request::builder().method(parts.method).uri(parts.uri).version(Version::HTTP_3);
        for (name, value) in &parts.headers {
            request_builder = request_builder.header(name, value);
        }
        let request = request_builder.body(()).map_err(|error| {
            Error::Server(format!("failed to build upstream http3 request: {error}"))
        })?;

        let mut request_stream = send_request.send_request(request).await.map_err(|error| {
            session.mark_closed();
            Error::Server(format!(
                "failed to send upstream http3 request headers to `{}`: {error}",
                peer.display_url
            ))
        })?;

        let mut finalized = false;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|error| {
                Error::Server(format!(
                    "failed to read downstream request body for upstream http3 `{}`: {error}",
                    peer.display_url
                ))
            })?;
            match frame.into_data() {
                Ok(data) => {
                    if !data.is_empty() {
                        request_stream.send_data(data).await.map_err(|error| {
                            session.mark_closed();
                            Error::Server(format!(
                                "failed to send upstream http3 request body to `{}`: {error}",
                                peer.display_url
                            ))
                        })?;
                    }
                }
                Err(frame) => {
                    if let Ok(trailers) = frame.into_trailers() {
                        request_stream.send_trailers(trailers).await.map_err(|error| {
                            session.mark_closed();
                            Error::Server(format!(
                                "failed to send upstream http3 request trailers to `{}`: {error}",
                                peer.display_url
                            ))
                        })?;
                        finalized = true;
                    }
                }
            }
        }
        if !finalized {
            request_stream.finish().await.map_err(|error| {
                session.mark_closed();
                Error::Server(format!(
                    "failed to finish upstream http3 request to `{}`: {error}",
                    peer.display_url
                ))
            })?;
        }

        let response = request_stream.recv_response().await.map_err(|error| {
            session.mark_closed();
            Error::Server(format!(
                "failed to receive upstream http3 response headers from `{}`: {error}",
                peer.display_url
            ))
        })?;

        let (parts, _) = response.into_parts();
        let size_hint = response_size_hint(&parts.headers);
        let body =
            streaming_response_body(request_stream, session, peer.display_url.clone(), size_hint);
        let mut response_builder = Response::builder().status(parts.status);
        for (name, value) in &parts.headers {
            response_builder = response_builder.header(name, value);
        }
        response_builder.body(body).map_err(|error| {
            Error::Server(format!("failed to build upstream http3 response: {error}"))
        })
    }
}
