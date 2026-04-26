use std::net::SocketAddr;

use rginx_core::Error;

use super::Http3Client;

#[derive(Default)]
pub(super) struct Http3ClientEndpoints {
    pub(super) ipv4: tokio::sync::Mutex<Option<quinn::Endpoint>>,
    pub(super) ipv6: tokio::sync::Mutex<Option<quinn::Endpoint>>,
}

impl Http3Client {
    pub(super) async fn endpoint_for_remote(
        &self,
        remote_addr: SocketAddr,
    ) -> Result<quinn::Endpoint, Error> {
        let cache = match remote_addr {
            SocketAddr::V4(_) => &self.endpoints.ipv4,
            SocketAddr::V6(_) => &self.endpoints.ipv6,
        };
        let mut endpoint = cache.lock().await;
        if let Some(endpoint) = endpoint.as_ref() {
            return Ok(endpoint.clone());
        }

        let bind_addr = match remote_addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
            SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
        };
        let mut created = quinn::Endpoint::client(bind_addr).map_err(Error::Io)?;
        created.set_default_client_config(self.client_config.clone());
        let reusable = created.clone();
        *endpoint = Some(created);
        Ok(reusable)
    }

    #[cfg(test)]
    pub(super) async fn cached_endpoint_count(&self) -> usize {
        usize::from(self.endpoints.ipv4.lock().await.is_some())
            + usize::from(self.endpoints.ipv6.lock().await.is_some())
    }

    #[cfg(test)]
    pub(super) async fn cached_endpoint_local_addr(
        &self,
        remote_addr: SocketAddr,
    ) -> Result<SocketAddr, Error> {
        self.endpoint_for_remote(remote_addr).await?.local_addr().map_err(Error::Io)
    }
}
