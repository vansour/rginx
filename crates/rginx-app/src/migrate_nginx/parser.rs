use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail};

use super::convert::ConvertedUpstreamVerify;
use super::tokenize::Token;

#[derive(Debug, Clone)]
pub(super) enum Statement {
    Directive { name: String, args: Vec<String> },
    Block { name: String, args: Vec<String>, children: Vec<Statement> },
}

pub(super) struct ParserState {
    tokens: Vec<Token>,
    cursor: usize,
}

impl ParserState {
    pub(super) fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, cursor: 0 }
    }

    pub(super) fn parse(mut self) -> Result<Vec<Statement>> {
        self.parse_statements(false)
    }

    fn parse_statements(&mut self, expect_rbrace: bool) -> Result<Vec<Statement>> {
        let mut statements = Vec::new();
        loop {
            match self.peek_text() {
                Some("}") if expect_rbrace => {
                    self.cursor += 1;
                    return Ok(statements);
                }
                Some("}") => {
                    let token = self.peek().expect("peek_text should imply peek");
                    bail!("unexpected `}}` on line {}", token.line);
                }
                Some(_) => statements.push(self.parse_statement()?),
                None if expect_rbrace => bail!("unexpected end of file while parsing nginx block"),
                None => return Ok(statements),
            }
        }
    }

    fn parse_statement(&mut self) -> Result<Statement> {
        let name = self.next_word("directive or block name")?;
        let mut args = Vec::new();

        loop {
            match self.peek_text() {
                Some(";") => {
                    self.cursor += 1;
                    return Ok(Statement::Directive { name: name.text, args });
                }
                Some("{") => {
                    self.cursor += 1;
                    let children = self.parse_statements(true)?;
                    return Ok(Statement::Block { name: name.text, args, children });
                }
                Some("}") => bail!("unexpected `}}` after `{}` on line {}", name.text, name.line),
                Some(_) => args.push(self.next_word("directive argument")?.text),
                None => bail!("unexpected end of file after `{}` on line {}", name.text, name.line),
            }
        }
    }

    fn next_word(&mut self, context: &str) -> Result<Token> {
        let token =
            self.tokens.get(self.cursor).cloned().ok_or_else(|| anyhow!("missing {context}"))?;
        if matches!(token.text.as_str(), "{" | "}" | ";") {
            bail!("expected {context} on line {}, got `{}`", token.line, token.text);
        }
        self.cursor += 1;
        Ok(token)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.cursor)
    }

    fn peek_text(&self) -> Option<&str> {
        self.peek().map(|token| token.text.as_str())
    }
}

#[derive(Debug, Default)]
pub(super) struct ParsedNginxConfig {
    pub(super) upstreams: Vec<ParsedUpstream>,
    pub(super) servers: Vec<ParsedServer>,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug)]
pub(super) struct ParsedUpstream {
    pub(super) name: String,
    pub(super) peers: Vec<ParsedUpstreamPeer>,
}

#[derive(Debug)]
pub(super) struct ParsedUpstreamPeer {
    pub(super) endpoint: String,
    pub(super) weight: u32,
    pub(super) backup: bool,
}

#[derive(Debug)]
pub(super) struct ParsedServer {
    pub(super) listens: Vec<ParsedListen>,
    pub(super) server_names: Vec<String>,
    pub(super) client_max_body_size: Option<u64>,
    pub(super) tls: ParsedServerTls,
    pub(super) locations: Vec<ParsedLocation>,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedListen {
    pub(super) address: String,
    pub(super) ssl: bool,
}

#[derive(Debug)]
pub(super) struct ParsedLocation {
    pub(super) matcher: ParsedMatcher,
    pub(super) proxy_pass: String,
    pub(super) preserve_host: bool,
    pub(super) proxy_set_headers: BTreeMap<String, String>,
    pub(super) client_max_body_size: Option<u64>,
    pub(super) upstream_tls: ParsedUpstreamTls,
}

#[derive(Debug, Default, Clone)]
pub(super) struct ParsedServerTls {
    pub(super) cert_paths: Vec<String>,
    pub(super) key_paths: Vec<String>,
    pub(super) versions: Vec<String>,
    pub(super) ocsp_staple_path: Option<String>,
    pub(super) session_tickets: Option<bool>,
    pub(super) client_ca_path: Option<String>,
    pub(super) client_auth_mode: Option<String>,
    pub(super) client_auth_verify_depth: Option<u32>,
    pub(super) client_auth_crl_path: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub(super) struct ParsedUpstreamTls {
    pub(super) verify: Option<ConvertedUpstreamVerify>,
    pub(super) versions: Vec<String>,
    pub(super) verify_depth: Option<u32>,
    pub(super) crl_path: Option<String>,
    pub(super) client_cert_path: Option<String>,
    pub(super) client_key_path: Option<String>,
    pub(super) server_name: Option<bool>,
    pub(super) server_name_override: Option<String>,
}

#[derive(Debug)]
pub(super) enum ParsedMatcher {
    Exact(String),
    Prefix(String),
}

impl ParsedNginxConfig {
    pub(super) fn from_statements(statements: Vec<Statement>) -> Result<Self> {
        let mut parsed = Self::default();
        let mut top_level = false;

        for statement in statements {
            match statement {
                Statement::Block { name, children, .. } if name == "http" => {
                    top_level = true;
                    parsed.parse_http_children(children)?;
                }
                Statement::Block { name, args, children } if name == "upstream" => {
                    top_level = true;
                    parsed.upstreams.push(parse_upstream_block(
                        &args,
                        children,
                        &mut parsed.warnings,
                    )?);
                }
                Statement::Block { name, args: _, children } if name == "server" => {
                    top_level = true;
                    let server = parse_server_block(children, &mut parsed.warnings)?;
                    if server.locations.is_empty() {
                        parsed.warnings.push(
                            "skipped an nginx `server` block because none of its locations were within the supported reverse-proxy migration subset"
                                .to_string(),
                        );
                    } else {
                        parsed.servers.push(server);
                    }
                }
                Statement::Directive { name, .. } => {
                    parsed.warnings.push(format!(
                        "ignored top-level nginx directive `{name}`; export the effective config with `nginx -T` before migrating if you rely on includes or globals"
                    ));
                }
                Statement::Block { name, .. } => {
                    parsed.warnings.push(format!(
                        "ignored top-level nginx block `{name}` because it is outside the supported reverse-proxy subset"
                    ));
                }
            }
        }

        if !top_level {
            bail!("no nginx `http`, `upstream`, or `server` blocks were found");
        }
        if parsed.servers.is_empty() {
            bail!("no nginx `server` blocks were found");
        }

        Ok(parsed)
    }

    fn parse_http_children(&mut self, children: Vec<Statement>) -> Result<()> {
        for child in children {
            match child {
                Statement::Block { name, args, children } if name == "upstream" => {
                    self.upstreams.push(parse_upstream_block(&args, children, &mut self.warnings)?);
                }
                Statement::Block { name, args: _, children } if name == "server" => {
                    let server = parse_server_block(children, &mut self.warnings)?;
                    if server.locations.is_empty() {
                        self.warnings.push(
                            "skipped an nginx `server` block because none of its locations were within the supported reverse-proxy migration subset"
                                .to_string(),
                        );
                    } else {
                        self.servers.push(server);
                    }
                }
                Statement::Directive { name, .. } => {
                    self.warnings.push(format!(
                        "ignored nginx `http` directive `{name}`; the migration helper only extracts reverse-proxy blocks"
                    ));
                }
                Statement::Block { name, .. } => {
                    self.warnings.push(format!(
                        "ignored nginx `http` block `{name}` because it is outside the supported migration subset"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn parse_upstream_block(
    args: &[String],
    children: Vec<Statement>,
    warnings: &mut Vec<String>,
) -> Result<ParsedUpstream> {
    if args.len() != 1 {
        bail!("nginx upstream blocks must have exactly one name");
    }

    let mut peers = Vec::new();
    for child in children {
        match child {
            Statement::Directive { name, args } if name == "server" => {
                peers.push(parse_upstream_peer(&args, warnings)?);
            }
            Statement::Directive { name, .. } => warnings.push(format!(
                "ignored nginx upstream directive `{name}` inside upstream `{}`",
                args[0]
            )),
            Statement::Block { name, .. } => warnings
                .push(format!("ignored nested nginx block `{name}` inside upstream `{}`", args[0])),
        }
    }

    if peers.is_empty() {
        bail!("nginx upstream `{}` does not contain any `server` entries", args[0]);
    }

    Ok(ParsedUpstream { name: args[0].clone(), peers })
}

fn parse_upstream_peer(args: &[String], warnings: &mut Vec<String>) -> Result<ParsedUpstreamPeer> {
    let endpoint =
        args.first().ok_or_else(|| anyhow!("nginx upstream server entries need an address"))?;
    if endpoint.starts_with("unix:") {
        bail!("unix upstream sockets are outside the supported Week 8 migration subset");
    }

    let mut weight = 1u32;
    let mut backup = false;

    for option in &args[1..] {
        if let Some(value) = option.strip_prefix("weight=") {
            weight = value
                .parse::<u32>()
                .with_context(|| format!("invalid upstream weight `{value}`"))?;
            continue;
        }
        if option == "backup" {
            backup = true;
            continue;
        }

        warnings.push(format!("ignored nginx upstream server option `{option}` for `{endpoint}`"));
    }

    Ok(ParsedUpstreamPeer { endpoint: endpoint.clone(), weight, backup })
}

fn parse_server_block(
    children: Vec<Statement>,
    warnings: &mut Vec<String>,
) -> Result<ParsedServer> {
    let mut listens = Vec::new();
    let mut server_names = Vec::new();
    let mut client_max_body_size = None;
    let mut tls = ParsedServerTls::default();
    let mut locations = Vec::new();

    for child in children {
        match child {
            Statement::Directive { name, args } if name == "listen" => {
                listens.push(parse_listen(&args, warnings)?);
            }
            Statement::Directive { name, args } if name == "server_name" => {
                server_names.extend(args);
            }
            Statement::Directive { name, args } if name == "client_max_body_size" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("client_max_body_size requires a value"))?;
                client_max_body_size = Some(parse_size(value)?);
            }
            Statement::Directive { name, args } if name == "ssl_certificate" => {
                if let Some(path) = args.first() {
                    tls.cert_paths.push(path.clone());
                }
            }
            Statement::Directive { name, args } if name == "ssl_certificate_key" => {
                if let Some(path) = args.first() {
                    tls.key_paths.push(path.clone());
                }
            }
            Statement::Directive { name, args } if name == "ssl_client_certificate" => {
                tls.client_ca_path = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "ssl_verify_depth" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("ssl_verify_depth requires a value"))?;
                tls.client_auth_verify_depth =
                    Some(parse_verify_depth(value, "ssl_verify_depth")?);
            }
            Statement::Directive { name, args } if name == "ssl_crl" => {
                tls.client_auth_crl_path = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "ssl_verify_client" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("ssl_verify_client requires a value"))?;
                tls.client_auth_mode = match value.as_str() {
                    "optional" => Some("Optional".to_string()),
                    "on" | "required" => Some("Required".to_string()),
                    "off" => None,
                    other => {
                        warnings.push(format!(
                            "nginx `ssl_verify_client {other}` is not migrated automatically; review downstream mTLS policy manually"
                        ));
                        None
                    }
                };
            }
            Statement::Directive { name, args } if name == "ssl_protocols" => {
                tls.versions = parse_tls_versions(&args, "ssl_protocols", warnings);
            }
            Statement::Directive { name, args } if name == "ssl_session_tickets" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("ssl_session_tickets requires a value"))?;
                tls.session_tickets =
                    Some(parse_nginx_on_off(value, "ssl_session_tickets", warnings));
            }
            Statement::Directive { name, args } if name == "ssl_stapling_file" => {
                tls.ocsp_staple_path = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "ssl_stapling" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("ssl_stapling requires a value"))?;
                if !matches!(value.as_str(), "off" | "false" | "0") {
                    warnings.push(
                        "nginx `ssl_stapling on` was detected; rginx can auto-refresh OCSP when the certificate exposes an AIA responder and `ocsp_staple_path` is configured, but this migration helper only maps `ssl_stapling_file` directly"
                            .to_string(),
                    );
                }
            }
            Statement::Block {
                name,
                args,
                children,
            } if name == "location" => {
                match parse_location(&args, children, warnings) {
                    Ok(location) => locations.push(location),
                    Err(error) => warnings.push(format!(
                        "skipped nginx location `{}` because it is outside the supported reverse-proxy migration subset: {error}",
                        args.join(" ")
                    )),
                }
            }
            Statement::Directive { name, .. } => warnings.push(format!(
                "ignored nginx server directive `{name}` because it is outside the supported migration subset"
            )),
            Statement::Block { name, .. } => warnings.push(format!(
                "ignored nested nginx block `{name}` inside a `server` block"
            )),
        }
    }

    if locations.is_empty() {
        bail!("nginx server blocks must contain at least one supported `location` block");
    }

    if listens.is_empty() {
        warnings.push(
            "nginx server block had no explicit `listen`; assuming `0.0.0.0:80` during migration"
                .to_string(),
        );
        listens.push(ParsedListen { address: "0.0.0.0:80".to_string(), ssl: false });
    }

    Ok(ParsedServer { listens, server_names, client_max_body_size, tls, locations })
}

fn parse_listen(args: &[String], warnings: &mut Vec<String>) -> Result<ParsedListen> {
    if args.is_empty() {
        bail!("nginx `listen` directives require an address");
    }

    let mut address = None;
    let mut ssl = false;
    for arg in args {
        match arg.as_str() {
            "default_server" | "http2" | "proxy_protocol" | "reuseport" | "deferred" | "bind"
            | "quic" => {
                if arg == "http2" {
                    warnings.push(
                        "nginx `listen ... http2` was seen; inbound HTTP/2 in rginx is negotiated via TLS/ALPN, so verify TLS placement manually"
                            .to_string(),
                    );
                }
                if arg == "proxy_protocol" {
                    warnings.push(
                        "nginx `listen ... proxy_protocol` was ignored by the migration tool; enable `proxy_protocol: Some(true)` manually on the target listener if needed"
                            .to_string(),
                    );
                }
                if arg == "quic" {
                    warnings.push(
                        "nginx `listen ... quic` was ignored by the migration tool because HTTP/3/QUIC is outside the supported migration subset"
                            .to_string(),
                    );
                }
            }
            "ssl" => ssl = true,
            option if option.contains('=') => {
                warnings.push(format!("ignored nginx `listen` option `{option}` during migration"))
            }
            candidate if address.is_none() => address = Some(normalize_listen_address(candidate)?),
            candidate => warnings
                .push(format!("ignored nginx `listen` token `{candidate}` during migration")),
        }
    }

    let address = address.unwrap_or_else(|| "0.0.0.0:80".to_string());
    Ok(ParsedListen { address, ssl })
}

fn normalize_listen_address(value: &str) -> Result<String> {
    if value.starts_with("unix:") {
        bail!("unix listeners are outside the supported Week 8 migration subset");
    }

    if value.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(format!("0.0.0.0:{value}"));
    }

    Ok(value.to_string())
}

fn parse_location(
    args: &[String],
    children: Vec<Statement>,
    warnings: &mut Vec<String>,
) -> Result<ParsedLocation> {
    let matcher = match args {
        [path] => ParsedMatcher::Prefix(path.clone()),
        [modifier, path] if modifier == "=" => ParsedMatcher::Exact(path.clone()),
        [modifier, path] if modifier == "^~" => ParsedMatcher::Prefix(path.clone()),
        [modifier, ..] if modifier == "~" || modifier == "~*" => {
            bail!("regex nginx locations are outside the supported Week 8 migration subset")
        }
        _ => bail!("unsupported nginx location syntax: expected `location /path {{ ... }}`"),
    };
    let matcher_path = match &matcher {
        ParsedMatcher::Exact(path) | ParsedMatcher::Prefix(path) => path,
    };
    if matcher_path.starts_with('@') {
        bail!("named nginx locations are outside the supported Week 8 migration subset");
    }

    let mut proxy_pass = None;
    let mut preserve_host = false;
    let mut proxy_set_headers = BTreeMap::new();
    let mut client_max_body_size = None;
    let mut upstream_tls = ParsedUpstreamTls::default();

    for child in children {
        match child {
            Statement::Directive { name, args } if name == "proxy_pass" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("proxy_pass requires an upstream target"))?;
                proxy_pass = Some(value.clone());
            }
            Statement::Directive { name, args } if name == "proxy_set_header" => {
                if args.len() < 2 {
                    bail!("proxy_set_header requires a name and a value");
                }
                let header_name = args[0].clone();
                let header_value = args[1..].join(" ");

                if header_name.eq_ignore_ascii_case("host")
                    && matches!(header_value.as_str(), "$host" | "$http_host")
                {
                    preserve_host = true;
                    continue;
                }

                if header_name.eq_ignore_ascii_case("x-forwarded-for")
                    && header_value == "$proxy_add_x_forwarded_for"
                {
                    continue;
                }

                if header_name.eq_ignore_ascii_case("x-forwarded-proto")
                    && header_value == "$scheme"
                {
                    continue;
                }

                if header_name.eq_ignore_ascii_case("x-forwarded-host")
                    && matches!(header_value.as_str(), "$host" | "$http_host")
                {
                    continue;
                }

                if header_value.starts_with('$') {
                    warnings.push(format!(
                        "skipped nginx `proxy_set_header {header_name} {header_value}` because rginx `proxy_set_headers` does not expand nginx variables"
                    ));
                    continue;
                }

                proxy_set_headers.insert(header_name, header_value);
            }
            Statement::Directive { name, args } if name == "client_max_body_size" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("client_max_body_size requires a value"))?;
                client_max_body_size = Some(parse_size(value)?);
                warnings.push(
                    "lifted a location-level `client_max_body_size` to server-level `max_request_body_bytes`; review this migration because rginx does not currently scope body limits per route"
                        .to_string(),
                );
            }
            Statement::Directive { name, args } if name == "proxy_ssl_verify" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("proxy_ssl_verify requires a value"))?;
                upstream_tls.verify = Some(match value.as_str() {
                    "off" => ConvertedUpstreamVerify::Insecure,
                    _ => ConvertedUpstreamVerify::NativeRoots,
                });
            }
            Statement::Directive { name, args } if name == "proxy_ssl_trusted_certificate" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("proxy_ssl_trusted_certificate requires a path"))?;
                upstream_tls.verify = Some(ConvertedUpstreamVerify::CustomCa(value.clone()));
            }
            Statement::Directive { name, args } if name == "proxy_ssl_certificate" => {
                upstream_tls.client_cert_path = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "proxy_ssl_certificate_key" => {
                upstream_tls.client_key_path = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "proxy_ssl_protocols" => {
                upstream_tls.versions =
                    parse_tls_versions(&args, "proxy_ssl_protocols", warnings);
            }
            Statement::Directive { name, args } if name == "proxy_ssl_verify_depth" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("proxy_ssl_verify_depth requires a value"))?;
                upstream_tls.verify_depth =
                    Some(parse_verify_depth(value, "proxy_ssl_verify_depth")?);
            }
            Statement::Directive { name, args } if name == "proxy_ssl_crl" => {
                upstream_tls.crl_path = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "proxy_ssl_name" => {
                upstream_tls.server_name_override = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "proxy_ssl_server_name" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("proxy_ssl_server_name requires a value"))?;
                upstream_tls.server_name =
                    Some(!matches!(value.as_str(), "off" | "false" | "0"));
            }
            Statement::Directive { name, .. } => warnings.push(format!(
                "ignored nginx location directive `{name}` because it is outside the supported migration subset"
            )),
            Statement::Block { name, .. } => warnings.push(format!(
                "ignored nested nginx block `{name}` inside a location because it is outside the supported migration subset"
            )),
        }
    }

    let proxy_pass = proxy_pass.ok_or_else(|| {
        anyhow!("only `proxy_pass` locations are supported by the Week 8 migration helper")
    })?;

    Ok(ParsedLocation {
        matcher,
        proxy_pass,
        preserve_host,
        proxy_set_headers,
        client_max_body_size,
        upstream_tls,
    })
}

pub(super) fn parse_size(raw: &str) -> Result<u64> {
    let lower = raw.trim().to_ascii_lowercase();
    let (number, multiplier) = match lower.chars().last() {
        Some('k') => (&lower[..lower.len() - 1], 1024u64),
        Some('m') => (&lower[..lower.len() - 1], 1024u64 * 1024),
        Some('g') => (&lower[..lower.len() - 1], 1024u64 * 1024 * 1024),
        Some(_) => (lower.as_str(), 1),
        None => bail!("empty size value"),
    };
    let value = number.parse::<u64>().with_context(|| format!("invalid size value `{raw}`"))?;
    value.checked_mul(multiplier).ok_or_else(|| anyhow!("size value `{raw}` exceeds u64 limits"))
}

fn parse_tls_versions(args: &[String], directive: &str, warnings: &mut Vec<String>) -> Vec<String> {
    let mut versions = Vec::new();
    for version in args {
        let mapped = match version.as_str() {
            "TLSv1.2" => Some("Tls12"),
            "TLSv1.3" => Some("Tls13"),
            "TLSv1" | "TLSv1.1" => None,
            _ => None,
        };
        if let Some(mapped) = mapped {
            if !versions.iter().any(|existing| existing == mapped) {
                versions.push(mapped.to_string());
            }
        } else {
            warnings.push(format!(
                "nginx `{directive} {version}` is not migrated because this rustls-based build only supports TLSv1.2 and TLSv1.3"
            ));
        }
    }
    versions
}

fn parse_verify_depth(raw: &str, directive: &str) -> Result<u32> {
    let depth =
        raw.parse::<u32>().with_context(|| format!("invalid `{directive}` value `{raw}`"))?;
    if depth == 0 {
        bail!("`{directive}` must be greater than 0");
    }
    Ok(depth)
}

fn parse_nginx_on_off(raw: &str, directive: &str, warnings: &mut Vec<String>) -> bool {
    match raw {
        "on" | "true" | "1" => true,
        "off" | "false" | "0" => false,
        other => {
            warnings.push(format!(
                "nginx `{directive} {other}` is not migrated exactly; keeping the default equivalent behavior"
            ));
            true
        }
    }
}
