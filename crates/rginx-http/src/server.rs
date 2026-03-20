use std::convert::Infallible;
use std::sync::Arc;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rginx_core::{ConfigSnapshot, Result};
use tokio::net::TcpListener;
use tokio::sync::watch;

pub async fn serve(
    listener: TcpListener,
    config: Arc<ConfigSnapshot>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let clients = crate::proxy::ProxyClients::from_config(config.as_ref())?;

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        tracing::info!("http accept loop stopping");
                        break;
                    }
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
            accepted = listener.accept() => {
                let (stream, remote_addr) = accepted?;
                let io = TokioIo::new(stream);
                let config = config.clone();
                let clients = clients.clone();

                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let config = config.clone();
                        let clients = clients.clone();
                        async move {
                            Ok::<_, Infallible>(
                                crate::handler::handle(request, config, clients, remote_addr).await
                            )
                        }
                    });

                    if let Err(error) = http1::Builder::new()
                        .keep_alive(true)
                        .serve_connection(io, service)
                        .await
                    {
                        tracing::warn!(remote_addr = %remote_addr, %error, "connection closed with error");
                    }
                });
            }
        }
    }

    Ok(())
}
