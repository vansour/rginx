use bytes::Bytes;
use hyper::body::{Frame, SizeHint};
use pin_project_lite::pin_project;

use crate::handler::BoxError;
use crate::proxy::HealthChangeNotifier;

use super::PeerHealth;

pub(crate) struct ActivePeerGuard {
    pub(super) peer: Option<std::sync::Arc<PeerHealth>>,
    pub(super) notifier: Option<HealthChangeNotifier>,
    pub(super) upstream_name: String,
}

impl Drop for ActivePeerGuard {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.take() {
            let transitioned_to_idle = peer.decrement_active_requests();
            if transitioned_to_idle && let Some(notifier) = &self.notifier {
                notifier(&self.upstream_name);
            }
        }
    }
}

pin_project! {
    pub(crate) struct ActivePeerBody<B> {
        #[pin]
        inner: B,
        guard: Option<ActivePeerGuard>,
    }
}

impl<B> ActivePeerBody<B> {
    pub(crate) fn new(inner: B, guard: ActivePeerGuard) -> Self {
        Self { inner, guard: Some(guard) }
    }
}

impl<B> hyper::body::Body for ActivePeerBody<B>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        match this.inner.as_mut().poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => std::task::Poll::Ready(Some(Ok(frame))),
            std::task::Poll::Ready(Some(Err(error))) => {
                this.guard.take();
                std::task::Poll::Ready(Some(Err(error.into())))
            }
            std::task::Poll::Ready(None) => {
                this.guard.take();
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
