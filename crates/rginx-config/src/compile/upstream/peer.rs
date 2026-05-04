use rginx_core::{Error, Result, UpstreamPeer};

pub(super) fn compile_peer(
    upstream_name: &str,
    url: String,
    weight: u32,
    backup: bool,
    max_conns: Option<u64>,
) -> Result<UpstreamPeer> {
    let uri: http::Uri = url.parse()?;
    let scheme = uri.scheme_str().ok_or_else(|| {
        Error::Config(format!("upstream `{upstream_name}` peer `{url}` must include a scheme"))
    })?;
    let authority = uri.authority().ok_or_else(|| {
        Error::Config(format!("upstream `{upstream_name}` peer `{url}` must include an authority"))
    })?;

    if scheme != "http" && scheme != "https" {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` peer `{url}` uses unsupported scheme `{scheme}`; only `http` and `https` are supported in this build"
        )));
    }

    if uri.path() != "/" && !uri.path().is_empty() {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` peer `{url}` must not contain a path"
        )));
    }

    if uri.query().is_some() {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` peer `{url}` must not contain a query"
        )));
    }

    Ok(UpstreamPeer {
        url,
        scheme: scheme.to_string(),
        authority: authority.to_string(),
        weight,
        backup,
        max_conns: max_conns
            .map(|value| {
                usize::try_from(value).map_err(|_| {
                    Error::Config(format!(
                        "upstream `{upstream_name}` peer `{authority}` max_conns `{value}` does not fit into usize"
                    ))
                })
            })
            .transpose()?,
    })
}
