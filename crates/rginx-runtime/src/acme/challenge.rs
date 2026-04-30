use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, RwLock as StdRwLock};

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{Method, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rginx_core::{ConfigSnapshot, Error, Result};
use rginx_http::SharedState;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::types::http01_listener_addrs;

const ACME_HTTP01_PREFIX: &str = "/.well-known/acme-challenge/";

pub(crate) trait ChallengeBackend: Send + Sync {
    fn register_http01(&self, token: String, key_authorization: String);
    fn unregister_http01(&self, token: &str);
}

#[derive(Clone)]
pub(crate) struct RuntimeChallengeBackend {
    state: SharedState,
}

impl RuntimeChallengeBackend {
    pub(crate) fn new(state: SharedState) -> Self {
        Self { state }
    }
}

impl ChallengeBackend for RuntimeChallengeBackend {
    fn register_http01(&self, token: String, key_authorization: String) {
        self.state.register_acme_http01_challenge(token, key_authorization);
    }

    fn unregister_http01(&self, token: &str) {
        self.state.unregister_acme_http01_challenge(token);
    }
}

pub(crate) struct TemporaryChallengeServer {
    backend: Arc<TemporaryChallengeStore>,
    shutdown_tx: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl TemporaryChallengeServer {
    pub(crate) async fn bind_for_config(config: &ConfigSnapshot) -> Result<Self> {
        let listen_addrs = http01_listener_addrs(config);
        if listen_addrs.is_empty() {
            return Err(Error::Config(
                "ACME HTTP-01 requires at least one plain HTTP listener on port 80".to_string(),
            ));
        }

        let backend = Arc::new(TemporaryChallengeStore::default());
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let mut tasks = Vec::with_capacity(listen_addrs.len());

        for listen_addr in listen_addrs {
            let listener = TcpListener::bind(listen_addr).await.map_err(|error| {
                Error::Server(format!(
                    "failed to bind temporary ACME HTTP-01 listener on {listen_addr}: {error}"
                ))
            })?;
            let store = backend.clone();
            let shutdown = shutdown_tx.subscribe();
            tasks.push(tokio::spawn(async move {
                run_listener(listener, store, shutdown).await;
            }));
        }

        Ok(Self { backend, shutdown_tx, tasks })
    }

    pub(crate) fn backend(&self) -> Arc<dyn ChallengeBackend> {
        self.backend.clone()
    }

    pub(crate) async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        for task in self.tasks {
            if let Err(error) = task.await
                && !error.is_cancelled()
            {
                tracing::debug!(%error, "temporary ACME HTTP-01 listener task exited unexpectedly");
            }
        }
    }
}

#[derive(Default)]
struct TemporaryChallengeStore {
    challenges: StdRwLock<HashMap<String, String>>,
}

impl ChallengeBackend for TemporaryChallengeStore {
    fn register_http01(&self, token: String, key_authorization: String) {
        self.challenges
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(token, key_authorization);
    }

    fn unregister_http01(&self, token: &str) {
        self.challenges.write().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(token);
    }
}

impl TemporaryChallengeStore {
    fn response(&self, token: &str) -> Option<String> {
        self.challenges.read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(token).cloned()
    }
}

async fn run_listener(
    listener: TcpListener,
    store: Arc<TemporaryChallengeStore>,
    mut shutdown: watch::Receiver<bool>,
) {
    let listen_addr = listener.local_addr().ok();

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                match changed {
                    Ok(()) if *shutdown.borrow() => break,
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _peer_addr)) => {
                        let store = store.clone();
                        tokio::spawn(async move {
                            let service = service_fn(move |request: Request<Incoming>| {
                                let store = store.clone();
                                async move { Ok::<_, Infallible>(build_response(request, store)) }
                            });
                            if let Err(error) = http1::Builder::new()
                                .serve_connection(TokioIo::new(stream), service)
                                .await
                            {
                                tracing::debug!(%error, "temporary ACME HTTP-01 connection failed");
                            }
                        });
                    }
                    Err(error) => {
                        if let Some(listen_addr) = listen_addr {
                            tracing::warn!(%error, %listen_addr, "temporary ACME HTTP-01 listener stopped accepting");
                        } else {
                            tracing::warn!(%error, "temporary ACME HTTP-01 listener stopped accepting");
                        }
                        break;
                    }
                }
            }
        }
    }
}

fn build_response(
    request: Request<Incoming>,
    store: Arc<TemporaryChallengeStore>,
) -> Response<Full<Bytes>> {
    let key_authorization = request
        .uri()
        .path()
        .strip_prefix(ACME_HTTP01_PREFIX)
        .filter(|token| !token.is_empty() && !token.contains('/'))
        .and_then(|token| store.response(token));

    match (request.method(), key_authorization) {
        (&Method::GET, Some(body)) => text_response(StatusCode::OK, body),
        (&Method::HEAD, Some(_)) => empty_response(StatusCode::OK),
        _ => empty_response(StatusCode::NOT_FOUND),
    }
}

fn text_response(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(body)))
        .expect("temporary ACME HTTP-01 response should build")
}

fn empty_response(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .expect("temporary ACME HTTP-01 response should build")
}
