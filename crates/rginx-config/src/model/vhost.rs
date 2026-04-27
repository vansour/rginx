use serde::{Deserialize, Deserializer};

use super::{Http3Config, LocationConfig, UpstreamConfig, VirtualHostTlsConfig};

#[derive(Debug, Clone, Deserialize)]
pub struct VirtualHostConfig {
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub listen: Vec<String>,
    #[serde(default, alias = "server_name", deserialize_with = "deserialize_string_list")]
    pub server_names: Vec<String>,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
    #[serde(default)]
    pub locations: Vec<LocationConfig>,
    #[serde(default)]
    pub tls: Option<VirtualHostTlsConfig>,
    #[serde(default)]
    pub http3: Option<Http3Config>,
}

fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrList {
        String(String),
        List(Vec<String>),
    }

    Ok(match StringOrList::deserialize(deserializer)? {
        StringOrList::String(value) => vec![value],
        StringOrList::List(values) => values,
    })
}
