use super::*;

pub(super) struct ReplayableRequestBody {
    body: Option<Bytes>,
    trailers: Option<HeaderMap>,
}

impl ReplayableRequestBody {
    pub(super) fn new(body: Bytes, trailers: Option<HeaderMap>) -> Self {
        Self { body: Some(body), trailers }
    }
}

impl hyper::body::Body for ReplayableRequestBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        if let Some(body) = this.body.take()
            && !body.is_empty()
        {
            return std::task::Poll::Ready(Some(Ok(Frame::data(body))));
        }

        if let Some(trailers) = this.trailers.take() {
            return std::task::Poll::Ready(Some(Ok(Frame::trailers(trailers))));
        }

        std::task::Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        self.body.as_ref().is_none_or(Bytes::is_empty) && self.trailers.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(self.body.as_ref().map_or(0, |body| body.len() as u64));
        hint
    }
}

pub(in crate::proxy) fn is_idempotent_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    )
}

pub(in crate::proxy) fn can_retry_peer_request(
    prepared_request: &PreparedProxyRequest,
    peer_count: usize,
    attempt_index: usize,
) -> bool {
    prepared_request.can_failover() && attempt_index + 1 < peer_count
}
