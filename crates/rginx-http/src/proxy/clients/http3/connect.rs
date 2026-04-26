use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use h3::client;
use rginx_core::{Error, Upstream};
use tokio::sync::Notify;

use crate::proxy::ResolvedUpstreamPeer;

use super::Http3Client;
use super::session::{Http3Session, Http3SessionEntry, Http3SessionKey};

impl Http3Client {
    pub(super) async fn session_for(
        &self,
        key: Http3SessionKey,
        peer_url: &str,
    ) -> Result<Arc<Http3Session>, Error> {
        loop {
            enum SessionAction {
                Wait(Arc<Notify>),
                Connect(Arc<Notify>),
            }

            let action = {
                let mut sessions = self.sessions.lock().await;
                match sessions.get(&key).cloned() {
                    Some(Http3SessionEntry::Ready(existing)) if !existing.is_closed() => {
                        return Ok(existing);
                    }
                    Some(Http3SessionEntry::Pending(notify)) => SessionAction::Wait(notify),
                    Some(Http3SessionEntry::Ready(_)) | None => {
                        let notify = Arc::new(Notify::new());
                        sessions.insert(key.clone(), Http3SessionEntry::Pending(notify.clone()));
                        SessionAction::Connect(notify)
                    }
                }
            };

            match action {
                SessionAction::Wait(notify) => notify.notified().await,
                SessionAction::Connect(notify) => {
                    let result = self.connect_session(&key, peer_url).await.map(Arc::new);
                    let mut sessions = self.sessions.lock().await;
                    match &result {
                        Ok(session) => {
                            sessions.insert(key.clone(), Http3SessionEntry::Ready(session.clone()));
                        }
                        Err(_) => {
                            sessions.remove(&key);
                        }
                    }
                    notify.notify_waiters();
                    return result;
                }
            }
        }
    }

    async fn connect_session(
        &self,
        key: &Http3SessionKey,
        peer_url: &str,
    ) -> Result<Http3Session, Error> {
        let endpoint = self.endpoint_for_remote(key.remote_addr).await?;
        let connecting = endpoint.connect(key.remote_addr, &key.server_name).map_err(|error| {
            Error::Server(format!(
                "failed to start upstream http3 connect to `{}`: {error}",
                peer_url
            ))
        })?;
        let connection = tokio::time::timeout(self.connect_timeout, connecting)
            .await
            .map_err(|_| {
                Error::Server(format!(
                    "upstream http3 connect to `{}` timed out after {} ms",
                    peer_url,
                    self.connect_timeout.as_millis()
                ))
            })?
            .map_err(|error| {
                Error::Server(format!("upstream http3 connect to `{}` failed: {error}", peer_url))
            })?;

        let (mut driver, send_request) =
            client::new(h3_quinn::Connection::new(connection)).await.map_err(|error| {
                Error::Server(format!(
                    "failed to initialize upstream http3 session for `{}`: {error}",
                    peer_url
                ))
            })?;
        let session = Http3Session::new(send_request);
        let driver_closed = session.close_flag();
        let driver_task = tokio::spawn(async move {
            let _ = poll_fn(|cx| driver.poll_close(cx)).await;
            driver_closed.store(true, Ordering::Release);
        });
        session.set_driver_task(driver_task).await;
        Ok(session)
    }

    #[cfg(test)]
    pub(super) async fn cached_session_count(&self) -> usize {
        self.sessions
            .lock()
            .await
            .values()
            .filter(|entry| matches!(entry, Http3SessionEntry::Ready(_)))
            .count()
    }
}

pub(super) fn server_name_for_peer(
    upstream: &Upstream,
    peer: &ResolvedUpstreamPeer,
) -> Result<String, Error> {
    if let Some(server_name_override) = upstream.server_name_override.as_ref() {
        return Ok(server_name_override.clone());
    }

    Ok(peer.server_name.clone())
}
