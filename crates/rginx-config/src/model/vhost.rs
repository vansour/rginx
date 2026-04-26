use serde::Deserialize;

use super::{LocationConfig, VirtualHostTlsConfig};

#[derive(Debug, Clone, Deserialize)]
pub struct VirtualHostConfig {
    #[serde(default)]
    pub server_names: Vec<String>,
    pub locations: Vec<LocationConfig>,
    #[serde(default)]
    pub tls: Option<VirtualHostTlsConfig>,
}
