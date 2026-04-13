use bytes::{Buf, Bytes, BytesMut};
use h3::client;
use http::{HeaderMap, Request, Response};
use std::net::{SocketAddr, ToSocketAddrs};

use crate::handler::{HttpBody, boxed_body};

use super::*;

#[derive(Clone)]
pub(crate) struct Http3Client {
    client_config: quinn::ClientConfig,
    connect_timeout: Duration,
}

impl Http3Client {
    pub(super) fn new(client_config: quinn::ClientConfig, connect_timeout: Duration) -> Self {
        Self { client_config, connect_timeout }
    }

    pub(super) async fn request(
        &self,
        upstream: &Upstream,
        peer: &UpstreamPeer,
        request: Request<HttpBody>,
    ) -> Result<Response<HttpBody>, Error> {
        let remote_addr = resolve_peer_socket_addr(peer)?;
        let endpoint_bind_addr = match remote_addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
            SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
        };
        let mut endpoint = quinn::Endpoint::client(endpoint_bind_addr).map_err(Error::Io)?;
        endpoint.set_default_client_config(self.client_config.clone());

        let server_name = server_name_for_peer(upstream, peer)?;
        let connecting = endpoint.connect(remote_addr, &server_name).map_err(|error| {
            Error::Server(format!(
                "failed to start upstream http3 connect to `{}`: {error}",
                peer.url
            ))
        })?;
        let connection = tokio::time::timeout(self.connect_timeout, connecting)
            .await
            .map_err(|_| {
                Error::Server(format!(
                    "upstream http3 connect to `{}` timed out after {} ms",
                    peer.url,
                    self.connect_timeout.as_millis()
                ))
            })?
            .map_err(|error| {
                Error::Server(format!("upstream http3 connect to `{}` failed: {error}", peer.url))
            })?;

        let (mut driver, mut send_request) =
            client::new(h3_quinn::Connection::new(connection)).await.map_err(|error| {
                Error::Server(format!(
                    "failed to initialize upstream http3 session for `{}`: {error}",
                    peer.url
                ))
            })?;
        let driver_task = tokio::spawn(async move {
            let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });

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
            Error::Server(format!(
                "failed to send upstream http3 request headers to `{}`: {error}",
                peer.url
            ))
        })?;

        let mut finalized = false;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|error| {
                Error::Server(format!(
                    "failed to read downstream request body for upstream http3 `{}`: {error}",
                    peer.url
                ))
            })?;
            match frame.into_data() {
                Ok(data) => {
                    if !data.is_empty() {
                        request_stream.send_data(data).await.map_err(|error| {
                            Error::Server(format!(
                                "failed to send upstream http3 request body to `{}`: {error}",
                                peer.url
                            ))
                        })?;
                    }
                }
                Err(frame) => {
                    if let Ok(trailers) = frame.into_trailers() {
                        request_stream.send_trailers(trailers).await.map_err(|error| {
                            Error::Server(format!(
                                "failed to send upstream http3 request trailers to `{}`: {error}",
                                peer.url
                            ))
                        })?;
                        finalized = true;
                    }
                }
            }
        }
        if !finalized {
            request_stream.finish().await.map_err(|error| {
                Error::Server(format!(
                    "failed to finish upstream http3 request to `{}`: {error}",
                    peer.url
                ))
            })?;
        }

        let response = request_stream.recv_response().await.map_err(|error| {
            Error::Server(format!(
                "failed to receive upstream http3 response headers from `{}`: {error}",
                peer.url
            ))
        })?;
        let mut buffered_body = BytesMut::new();
        while let Some(mut chunk) = request_stream.recv_data().await.map_err(|error| {
            Error::Server(format!(
                "failed to receive upstream http3 response body from `{}`: {error}",
                peer.url
            ))
        })? {
            buffered_body.extend_from_slice(chunk.copy_to_bytes(chunk.remaining()).as_ref());
        }
        let buffered_trailers = request_stream.recv_trailers().await.map_err(|error| {
            Error::Server(format!(
                "failed to receive upstream http3 response trailers from `{}`: {error}",
                peer.url
            ))
        })?;
        driver_task.abort();
        drop(endpoint);

        let (parts, _) = response.into_parts();
        let mut response_builder = Response::builder().status(parts.status);
        for (name, value) in &parts.headers {
            response_builder = response_builder.header(name, value);
        }
        response_builder
            .body(boxed_body(BufferedResponseBody::new(buffered_body.freeze(), buffered_trailers)))
            .map_err(|error| {
                Error::Server(format!("failed to build upstream http3 response: {error}"))
            })
    }
}

struct BufferedResponseBody {
    body: Option<Bytes>,
    trailers: Option<HeaderMap>,
}

impl BufferedResponseBody {
    fn new(body: Bytes, trailers: Option<HeaderMap>) -> Self {
        Self { body: (!body.is_empty()).then_some(body), trailers }
    }
}

impl hyper::body::Body for BufferedResponseBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        if let Some(body) = this.body.take() {
            return std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(body))));
        }

        if let Some(trailers) = this.trailers.take() {
            return std::task::Poll::Ready(Some(Ok(hyper::body::Frame::trailers(trailers))));
        }

        std::task::Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_none() && self.trailers.is_none()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        let mut hint = hyper::body::SizeHint::default();
        if let Some(body) = &self.body {
            hint.set_exact(body.len() as u64);
        }
        hint
    }
}

fn resolve_peer_socket_addr(peer: &UpstreamPeer) -> Result<SocketAddr, Error> {
    peer.authority
        .to_socket_addrs()
        .map_err(|error| {
            Error::Server(format!(
                "failed to resolve upstream http3 peer authority `{}`: {error}",
                peer.authority
            ))
        })?
        .next()
        .ok_or_else(|| {
            Error::Server(format!(
                "upstream http3 peer authority `{}` did not resolve to any address",
                peer.authority
            ))
        })
}

fn server_name_for_peer(upstream: &Upstream, peer: &UpstreamPeer) -> Result<String, Error> {
    if let Some(server_name_override) = upstream.server_name_override.as_ref() {
        return Ok(server_name_override.clone());
    }

    peer.url.parse::<http::Uri>().ok().and_then(|uri| uri.host().map(str::to_string)).ok_or_else(
        || {
            Error::Server(format!(
                "failed to derive TLS server name for upstream http3 peer `{}`",
                peer.url
            ))
        },
    )
}
