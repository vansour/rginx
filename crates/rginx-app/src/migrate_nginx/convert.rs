use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result, anyhow, bail};

use super::parser::{
    ParsedListen, ParsedLocation, ParsedMatcher, ParsedNginxConfig, ParsedServer, ParsedServerTls,
    ParsedUpstreamPeer, ParsedUpstreamTls,
};

fn convert_server_tls(
    tls: &ParsedServerTls,
    warnings: &mut Vec<String>,
) -> Option<ConvertedServerTls> {
    let (Some(cert_path), Some(key_path)) = (tls.cert_path.clone(), tls.key_path.clone()) else {
        if tls.cert_path.is_some()
            || tls.key_path.is_some()
            || !tls.versions.is_empty()
            || tls.ocsp_staple_path.is_some()
            || tls.session_tickets.is_some()
            || tls.client_auth_mode.is_some()
            || tls.client_auth_verify_depth.is_some()
            || tls.client_auth_crl_path.is_some()
        {
            warnings.push(
                "nginx downstream SSL directives were detected but certificate/key were incomplete; review `server.tls` manually after migration"
                    .to_string(),
            );
        }
        return None;
    };

    Some(ConvertedServerTls {
        cert_path,
        key_path,
        versions: tls.versions.clone(),
        ocsp_staple_path: tls.ocsp_staple_path.clone(),
        session_tickets: tls.session_tickets,
        client_ca_path: tls.client_ca_path.clone(),
        client_auth_mode: tls.client_auth_mode.clone(),
        client_auth_verify_depth: tls.client_auth_verify_depth,
        client_auth_crl_path: tls.client_auth_crl_path.clone(),
    })
}

fn convert_vhost_tls(
    tls: &ParsedServerTls,
    server_names: &[String],
    warnings: &mut Vec<String>,
) -> Option<ConvertedVhostTls> {
    let (Some(cert_path), Some(key_path)) = (tls.cert_path.clone(), tls.key_path.clone()) else {
        return None;
    };
    if !tls.versions.is_empty()
        || tls.ocsp_staple_path.is_some()
        || tls.session_tickets.is_some()
        || tls.client_ca_path.is_some()
        || tls.client_auth_mode.is_some()
        || tls.client_auth_verify_depth.is_some()
        || tls.client_auth_crl_path.is_some()
    {
        warnings.push(format!(
            "nginx SSL policy for server_names {} was migrated only as a certificate override; move protocol or client-auth policy onto `server.tls` or `listeners[].tls` manually",
            ron_string_list(server_names)
        ));
    }
    Some(ConvertedVhostTls { cert_path, key_path })
}

fn convert_upstream_tls(tls: &ParsedUpstreamTls) -> Option<ConvertedUpstreamTls> {
    if tls.verify.is_none()
        && tls.versions.is_empty()
        && tls.client_cert_path.is_none()
        && tls.client_key_path.is_none()
    {
        return None;
    }
    Some(ConvertedUpstreamTls {
        verify: tls.verify.clone().unwrap_or(ConvertedUpstreamVerify::NativeRoots),
        versions: tls.versions.clone(),
        verify_depth: tls.verify_depth,
        crl_path: tls.crl_path.clone(),
        client_cert_path: tls.client_cert_path.clone(),
        client_key_path: tls.client_key_path.clone(),
    })
}

fn merge_converted_upstream_tls(
    current: &mut Option<ConvertedUpstreamTls>,
    incoming: &ParsedUpstreamTls,
    upstream_name: &str,
    warnings: &mut Vec<String>,
) {
    let mut parsed = ParsedUpstreamTls::default();
    if let Some(existing) = current.as_ref() {
        parsed.verify = Some(existing.verify.clone());
        parsed.versions = existing.versions.clone();
        parsed.verify_depth = existing.verify_depth;
        parsed.crl_path = existing.crl_path.clone();
        parsed.client_cert_path = existing.client_cert_path.clone();
        parsed.client_key_path = existing.client_key_path.clone();
    }
    merge_upstream_tls(&mut parsed, incoming, upstream_name, warnings);
    *current = convert_upstream_tls(&parsed);
}

fn merge_upstream_tls(
    current: &mut ParsedUpstreamTls,
    incoming: &ParsedUpstreamTls,
    upstream_name: &str,
    warnings: &mut Vec<String>,
) {
    if let Some(verify) = incoming.verify.as_ref() {
        match current.verify.as_ref() {
            Some(existing) if existing != verify => warnings.push(format!(
                "conflicting nginx `proxy_ssl_verify` settings were found for upstream `{upstream_name}`; keeping the first migrated value"
            )),
            None => current.verify = Some(verify.clone()),
            _ => {}
        }
    }

    if !incoming.versions.is_empty() {
        if current.versions.is_empty() {
            current.versions = incoming.versions.clone();
        } else if current.versions != incoming.versions {
            warnings.push(format!(
                "conflicting nginx `proxy_ssl_protocols` values were found for upstream `{upstream_name}`; keeping the first migrated value"
            ));
        }
    }

    if let Some(verify_depth) = incoming.verify_depth {
        match current.verify_depth {
            Some(existing) if existing != verify_depth => warnings.push(format!(
                "conflicting nginx `proxy_ssl_verify_depth` values were found for upstream `{upstream_name}`; keeping the first migrated value"
            )),
            None => current.verify_depth = Some(verify_depth),
            _ => {}
        }
    }

    merge_optional_string(
        &mut current.crl_path,
        incoming.crl_path.as_ref(),
        upstream_name,
        "proxy_ssl_crl",
        warnings,
    );

    merge_optional_string(
        &mut current.client_cert_path,
        incoming.client_cert_path.as_ref(),
        upstream_name,
        "proxy_ssl_certificate",
        warnings,
    );
    merge_optional_string(
        &mut current.client_key_path,
        incoming.client_key_path.as_ref(),
        upstream_name,
        "proxy_ssl_certificate_key",
        warnings,
    );
}

fn merge_optional_string(
    current: &mut Option<String>,
    incoming: Option<&String>,
    upstream_name: &str,
    directive: &str,
    warnings: &mut Vec<String>,
) {
    let Some(incoming) = incoming else {
        return;
    };
    match current.as_ref() {
        Some(existing) if existing != incoming => warnings.push(format!(
            "conflicting nginx `{directive}` values were found for upstream `{upstream_name}`; keeping the first migrated value"
        )),
        None => *current = Some(incoming.clone()),
        _ => {}
    }
}

fn merge_optional_bool(
    current: &mut Option<bool>,
    incoming: Option<bool>,
    upstream_name: &str,
    directive: &str,
    warnings: &mut Vec<String>,
) {
    let Some(incoming) = incoming else {
        return;
    };
    match current {
        Some(existing) if *existing != incoming => warnings.push(format!(
            "conflicting nginx `{directive}` values were found for upstream `{upstream_name}`; keeping the first migrated value"
        )),
        None => *current = Some(incoming),
        _ => {}
    }
}

#[derive(Debug)]
pub(super) struct ConvertedConfig {
    pub(super) listeners: Vec<ConvertedListener>,
    pub(super) server: ConvertedServer,
    pub(super) vhosts: Vec<ConvertedVhost>,
    pub(super) upstreams: Vec<ConvertedUpstream>,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug)]
pub(super) struct ConvertedListener {
    pub(super) name: String,
    pub(super) listen: String,
    pub(super) tls: Option<ConvertedServerTls>,
}

#[derive(Debug)]
pub(super) struct ConvertedServer {
    pub(super) listen: Option<String>,
    pub(super) server_names: Vec<String>,
    pub(super) max_request_body_bytes: Option<u64>,
    pub(super) tls: Option<ConvertedServerTls>,
    pub(super) locations: Vec<ConvertedLocation>,
}

#[derive(Debug)]
pub(super) struct ConvertedVhost {
    pub(super) server_names: Vec<String>,
    pub(super) tls: Option<ConvertedVhostTls>,
    pub(super) locations: Vec<ConvertedLocation>,
}

#[derive(Debug)]
pub(super) struct ConvertedUpstream {
    pub(super) name: String,
    pub(super) peers: Vec<ConvertedPeer>,
    pub(super) tls: Option<ConvertedUpstreamTls>,
    pub(super) server_name: Option<bool>,
    pub(super) server_name_override: Option<String>,
}

#[derive(Debug)]
pub(super) struct ConvertedPeer {
    pub(super) url: String,
    pub(super) weight: u32,
    pub(super) backup: bool,
}

#[derive(Debug)]
pub(super) struct ConvertedLocation {
    pub(super) matcher: ConvertedMatcher,
    pub(super) upstream_name: String,
    pub(super) preserve_host: bool,
    pub(super) proxy_set_headers: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(super) enum ConvertedMatcher {
    Exact(String),
    Prefix(String),
}

#[derive(Debug)]
struct PendingUpstream {
    name: String,
    peers: Vec<ParsedUpstreamPeer>,
    resolved_scheme: Option<String>,
    tls: ParsedUpstreamTls,
    server_name: Option<bool>,
    server_name_override: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ConvertedServerTls {
    pub(super) cert_path: String,
    pub(super) key_path: String,
    pub(super) versions: Vec<String>,
    pub(super) ocsp_staple_path: Option<String>,
    pub(super) session_tickets: Option<bool>,
    pub(super) client_ca_path: Option<String>,
    pub(super) client_auth_mode: Option<String>,
    pub(super) client_auth_verify_depth: Option<u32>,
    pub(super) client_auth_crl_path: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ConvertedVhostTls {
    pub(super) cert_path: String,
    pub(super) key_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConvertedUpstreamVerify {
    NativeRoots,
    CustomCa(String),
    Insecure,
}

#[derive(Debug, Clone)]
pub(super) struct ConvertedUpstreamTls {
    pub(super) verify: ConvertedUpstreamVerify,
    pub(super) versions: Vec<String>,
    pub(super) verify_depth: Option<u32>,
    pub(super) crl_path: Option<String>,
    pub(super) client_cert_path: Option<String>,
    pub(super) client_key_path: Option<String>,
}

impl ConvertedConfig {
    pub(super) fn from_parsed(parsed: ParsedNginxConfig) -> Result<Self> {
        let mut warnings = dedupe_warnings(parsed.warnings);
        let mut pending_upstreams = parsed
            .upstreams
            .into_iter()
            .map(|upstream| {
                (
                    upstream.name.clone(),
                    PendingUpstream {
                        name: upstream.name,
                        peers: upstream.peers,
                        resolved_scheme: None,
                        tls: ParsedUpstreamTls::default(),
                        server_name: None,
                        server_name_override: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut implicit_upstreams = BTreeMap::<String, ConvertedUpstream>::new();
        let mut implicit_upstream_names = HashMap::<String, String>::new();

        let unique_listens = collect_unique_listens(&parsed.servers);
        if unique_listens.len() > 1 && parsed.servers.len() > 1 {
            warnings.push(
                "multiple nginx server blocks use distinct `listen` addresses; rginx listeners apply to the whole vhost set, so verify listener-to-vhost scoping manually after migration"
                    .to_string(),
            );
        }
        if unique_listens.iter().any(|listen| listen.ssl) {
            warnings.push(
                "nginx SSL listener flags were detected; copy TLS certificate/key settings into `server.tls`, `listeners[].tls`, or `servers[].tls` manually because the migration helper only covers the reverse-proxy subset"
                    .to_string(),
            );
        }

        let max_request_body_bytes = parsed
            .servers
            .iter()
            .flat_map(|server| {
                std::iter::once(server.client_max_body_size)
                    .chain(server.locations.iter().map(|location| location.client_max_body_size))
            })
            .flatten()
            .max();

        let distinct_body_limits = parsed
            .servers
            .iter()
            .flat_map(|server| {
                std::iter::once(server.client_max_body_size)
                    .chain(server.locations.iter().map(|location| location.client_max_body_size))
            })
            .flatten()
            .collect::<HashSet<_>>();
        if distinct_body_limits.len() > 1 {
            warnings.push(
                "multiple nginx `client_max_body_size` values were found; the migration helper lifted the largest value to `server.max_request_body_bytes` because rginx only supports a listener/server-scoped body limit today"
                    .to_string(),
            );
        }

        let server_count = parsed.servers.len();
        let mut servers = parsed.servers.into_iter();
        let default_server =
            servers.next().expect("parsed config should contain at least one server block");
        let default_server_tls = default_server.tls.clone();
        let default_locations = convert_locations(
            default_server.locations,
            &mut pending_upstreams,
            &mut implicit_upstreams,
            &mut implicit_upstream_names,
            &mut warnings,
        )?;
        let vhosts = servers
            .map(|server| {
                let tls = convert_vhost_tls(&server.tls, &server.server_names, &mut warnings);
                Ok(ConvertedVhost {
                    server_names: server.server_names,
                    tls,
                    locations: convert_locations(
                        server.locations,
                        &mut pending_upstreams,
                        &mut implicit_upstreams,
                        &mut implicit_upstream_names,
                        &mut warnings,
                    )?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let upstreams = finalize_upstreams(pending_upstreams, implicit_upstreams, &mut warnings)?;

        let listeners = if unique_listens.len() > 1 {
            unique_listens
                .into_iter()
                .enumerate()
                .map(|(index, listen)| {
                    let tls = if listen.ssl && server_count == 1 {
                        convert_server_tls(&default_server_tls, &mut warnings)
                    } else {
                        None
                    };
                    if listen.ssl && server_count > 1 {
                        warnings.push(
                            "nginx SSL settings across multiple server/listen combinations require manual placement on `listeners[].tls` after migration"
                                .to_string(),
                        );
                    }
                    ConvertedListener {
                        name: listener_name(index, &listen.address),
                        listen: listen.address,
                        tls,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let listeners_empty = listeners.is_empty();

        let listen = if listeners.is_empty() {
            Some(
                default_server
                    .listens
                    .first()
                    .map(|listen| listen.address.clone())
                    .unwrap_or_else(|| "0.0.0.0:80".to_string()),
            )
        } else {
            None
        };

        Ok(Self {
            listeners,
            server: ConvertedServer {
                listen,
                server_names: default_server.server_names,
                max_request_body_bytes,
                tls: if listeners_empty {
                    convert_server_tls(&default_server_tls, &mut warnings)
                } else {
                    None
                },
                locations: default_locations,
            },
            vhosts,
            upstreams,
            warnings: dedupe_warnings(warnings),
        })
    }
}

fn collect_unique_listens(servers: &[ParsedServer]) -> Vec<ParsedListen> {
    let mut seen = HashSet::new();
    let mut listens = Vec::new();
    for server in servers {
        for listen in &server.listens {
            if seen.insert((listen.address.clone(), listen.ssl)) {
                listens.push(listen.clone());
            }
        }
    }
    listens
}

fn convert_locations(
    locations: Vec<ParsedLocation>,
    pending_upstreams: &mut BTreeMap<String, PendingUpstream>,
    implicit_upstreams: &mut BTreeMap<String, ConvertedUpstream>,
    implicit_upstream_names: &mut HashMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<Vec<ConvertedLocation>> {
    locations
        .into_iter()
        .enumerate()
        .map(|(index, location)| {
            let target = parse_proxy_pass_target(&location.proxy_pass)?;
            let upstream_name = if let Some(upstream) = pending_upstreams.get_mut(&target.authority)
            {
                if let Some(existing) = &upstream.resolved_scheme {
                    if existing != &target.scheme {
                        warnings.push(format!(
                            "nginx upstream `{}` was referenced with both `{existing}` and `{}`; the migration helper kept `{existing}`",
                            upstream.name, target.scheme
                        ));
                    }
                } else {
                    upstream.resolved_scheme = Some(target.scheme.clone());
                }
                upstream.name.clone()
            } else {
                let key = format!("{}://{}", target.scheme, target.authority);
                if let Some(existing) = implicit_upstream_names.get(&key) {
                    existing.clone()
                } else {
                    let name = implicit_upstream_name(
                        index,
                        &target.authority,
                        implicit_upstream_names.len(),
                    );
                    implicit_upstream_names.insert(key, name.clone());
                    implicit_upstreams.insert(
                        name.clone(),
                        ConvertedUpstream {
                            name: name.clone(),
                            peers: vec![ConvertedPeer {
                                url: format!("{}://{}", target.scheme, target.authority),
                                weight: 1,
                                backup: false,
                            }],
                            tls: None,
                            server_name: None,
                            server_name_override: None,
                        },
                    );
                    name
                }
            };

            if let Some(upstream) = pending_upstreams.get_mut(&upstream_name) {
                merge_upstream_tls(
                    &mut upstream.tls,
                    &location.upstream_tls,
                    &upstream.name,
                    warnings,
                );
                merge_optional_bool(
                    &mut upstream.server_name,
                    location.upstream_tls.server_name,
                    &upstream.name,
                    "proxy_ssl_server_name",
                    warnings,
                );
                merge_optional_string(
                    &mut upstream.server_name_override,
                    location.upstream_tls.server_name_override.as_ref(),
                    &upstream.name,
                    "proxy_ssl_name",
                    warnings,
                );
            } else if let Some(upstream) = implicit_upstreams.get_mut(&upstream_name) {
                merge_converted_upstream_tls(
                    &mut upstream.tls,
                    &location.upstream_tls,
                    &upstream.name,
                    warnings,
                );
                merge_optional_bool(
                    &mut upstream.server_name,
                    location.upstream_tls.server_name,
                    &upstream.name,
                    "proxy_ssl_server_name",
                    warnings,
                );
                merge_optional_string(
                    &mut upstream.server_name_override,
                    location.upstream_tls.server_name_override.as_ref(),
                    &upstream.name,
                    "proxy_ssl_name",
                    warnings,
                );
            }

            Ok(ConvertedLocation {
                matcher: match location.matcher {
                    ParsedMatcher::Exact(path) => ConvertedMatcher::Exact(path),
                    ParsedMatcher::Prefix(path) => ConvertedMatcher::Prefix(path),
                },
                upstream_name,
                preserve_host: location.preserve_host,
                proxy_set_headers: location.proxy_set_headers,
            })
        })
        .collect()
}

fn finalize_upstreams(
    pending_upstreams: BTreeMap<String, PendingUpstream>,
    implicit_upstreams: BTreeMap<String, ConvertedUpstream>,
    warnings: &mut Vec<String>,
) -> Result<Vec<ConvertedUpstream>> {
    let mut upstreams = Vec::new();

    for pending in pending_upstreams.into_values() {
        let Some(scheme) = pending.resolved_scheme.clone() else {
            warnings.push(format!(
                "nginx upstream `{}` was not referenced by any migrated location and was skipped",
                pending.name
            ));
            continue;
        };

        let peers = pending
            .peers
            .into_iter()
            .map(|peer| {
                Ok(ConvertedPeer {
                    url: peer_url(&peer.endpoint, &scheme, warnings)?,
                    weight: peer.weight,
                    backup: peer.backup,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        upstreams.push(ConvertedUpstream {
            name: pending.name,
            peers,
            tls: convert_upstream_tls(&pending.tls),
            server_name: pending.server_name,
            server_name_override: pending.server_name_override,
        });
    }

    upstreams.extend(implicit_upstreams.into_values());
    upstreams.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(upstreams)
}

fn peer_url(endpoint: &str, scheme: &str, warnings: &mut Vec<String>) -> Result<String> {
    if endpoint.contains("://") {
        return Ok(endpoint.to_string());
    }

    if has_explicit_port(endpoint) {
        return Ok(format!("{scheme}://{endpoint}"));
    }

    let default_port = if scheme == "https" { 443 } else { 80 };
    warnings.push(format!(
        "assumed default port {default_port} for nginx upstream peer `{endpoint}` because no explicit port was set"
    ));
    Ok(format!("{scheme}://{endpoint}:{default_port}"))
}

fn has_explicit_port(endpoint: &str) -> bool {
    if endpoint.starts_with('[') {
        return endpoint.contains("]:");
    }
    endpoint.matches(':').count() == 1
}

#[derive(Debug)]
pub(super) struct ProxyPassTarget {
    scheme: String,
    authority: String,
}

pub(super) fn parse_proxy_pass_target(raw: &str) -> Result<ProxyPassTarget> {
    let uri: http::Uri =
        raw.parse().with_context(|| format!("invalid proxy_pass target `{raw}`"))?;
    let scheme = uri
        .scheme_str()
        .ok_or_else(|| anyhow!("proxy_pass target `{raw}` must include a scheme"))?;
    if !matches!(scheme, "http" | "https") {
        bail!("proxy_pass target `{raw}` uses unsupported scheme `{scheme}`");
    }
    let authority = uri
        .authority()
        .ok_or_else(|| anyhow!("proxy_pass target `{raw}` must include an authority"))?;
    if uri.path() != "/" && !uri.path().is_empty() {
        bail!(
            "proxy_pass target `{raw}` contains a URI path; review this location manually because rginx upstream peers do not currently carry a path component"
        );
    }
    if uri.query().is_some() {
        bail!(
            "proxy_pass target `{raw}` contains a query string; review this location manually because rginx upstream peers do not currently carry a query component"
        );
    }

    Ok(ProxyPassTarget { scheme: scheme.to_string(), authority: authority.to_string() })
}

fn listener_name(index: usize, address: &str) -> String {
    let mut sanitized = address
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    while sanitized.contains("__") {
        sanitized = sanitized.replace("__", "_");
    }
    format!("listener_{}_{}", index + 1, sanitized.trim_matches('_'))
}

fn implicit_upstream_name(index: usize, authority: &str, existing: usize) -> String {
    let mut sanitized = authority
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    while sanitized.contains("__") {
        sanitized = sanitized.replace("__", "_");
    }
    format!("imported_upstream_{}_{}_{}", existing + 1, index + 1, sanitized.trim_matches('_'))
}

fn dedupe_warnings(warnings: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for warning in warnings {
        if seen.insert(warning.clone()) {
            deduped.push(warning);
        }
    }
    deduped
}

fn ron_string_list(values: &[String]) -> String {
    let entries = values.iter().map(|value| format!("{value:?}")).collect::<Vec<_>>().join(", ");
    format!("[{entries}]")
}
