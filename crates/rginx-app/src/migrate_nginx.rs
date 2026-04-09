use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug)]
pub struct MigrationOutput {
    pub ron: String,
    pub warnings: Vec<String>,
}

pub fn migrate_file(input_path: &Path) -> Result<MigrationOutput> {
    let source = fs::read_to_string(input_path)
        .with_context(|| format!("failed to read nginx config {}", input_path.display()))?;
    migrate_source(&source, &input_path.display().to_string())
}

fn migrate_source(source: &str, source_label: &str) -> Result<MigrationOutput> {
    let tokens = tokenize(source)?;
    let statements = ParserState::new(tokens).parse()?;
    let parsed = ParsedNginxConfig::from_statements(statements)?;
    let converted = ConvertedConfig::from_parsed(parsed)?;
    Ok(MigrationOutput { ron: converted.render(source_label), warnings: converted.warnings })
}

#[derive(Debug, Clone)]
enum Statement {
    Directive { name: String, args: Vec<String> },
    Block { name: String, args: Vec<String>, children: Vec<Statement> },
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
    line: usize,
}

fn tokenize(source: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    let mut line = 1usize;

    while let Some(ch) = chars.next() {
        match ch {
            '\n' => line += 1,
            '#' => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        line += 1;
                        break;
                    }
                }
            }
            '{' | '}' | ';' => tokens.push(Token { text: ch.to_string(), line }),
            '"' | '\'' => {
                let quote = ch;
                let start_line = line;
                let mut value = String::new();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    match next {
                        '\n' => {
                            line += 1;
                            value.push('\n');
                        }
                        '\\' => {
                            let escaped = chars.next().ok_or_else(|| {
                                anyhow!("unterminated escape sequence on line {line}")
                            })?;
                            if escaped == '\n' {
                                line += 1;
                            }
                            value.push(escaped);
                        }
                        candidate if candidate == quote => {
                            closed = true;
                            break;
                        }
                        candidate => value.push(candidate),
                    }
                }
                if !closed {
                    bail!("unterminated quoted string starting on line {start_line}");
                }
                tokens.push(Token { text: value, line: start_line });
            }
            whitespace if whitespace.is_whitespace() => {}
            other => {
                let mut value = String::from(other);
                while let Some(peek) = chars.peek().copied() {
                    if peek.is_whitespace() || matches!(peek, '{' | '}' | ';' | '#') {
                        break;
                    }
                    value.push(peek);
                    chars.next();
                }
                tokens.push(Token { text: value, line });
            }
        }
    }

    Ok(tokens)
}

struct ParserState {
    tokens: Vec<Token>,
    cursor: usize,
}

impl ParserState {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, cursor: 0 }
    }

    fn parse(mut self) -> Result<Vec<Statement>> {
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
struct ParsedNginxConfig {
    upstreams: Vec<ParsedUpstream>,
    servers: Vec<ParsedServer>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct ParsedUpstream {
    name: String,
    peers: Vec<ParsedUpstreamPeer>,
}

#[derive(Debug)]
struct ParsedUpstreamPeer {
    endpoint: String,
    weight: u32,
    backup: bool,
}

#[derive(Debug)]
struct ParsedServer {
    listens: Vec<ParsedListen>,
    server_names: Vec<String>,
    client_max_body_size: Option<u64>,
    tls: ParsedServerTls,
    locations: Vec<ParsedLocation>,
}

#[derive(Debug, Clone)]
struct ParsedListen {
    address: String,
    ssl: bool,
}

#[derive(Debug)]
struct ParsedLocation {
    matcher: ParsedMatcher,
    proxy_pass: String,
    preserve_host: bool,
    proxy_set_headers: BTreeMap<String, String>,
    client_max_body_size: Option<u64>,
    upstream_tls: ParsedUpstreamTls,
}

#[derive(Debug, Default, Clone)]
struct ParsedServerTls {
    cert_path: Option<String>,
    key_path: Option<String>,
    versions: Vec<String>,
    client_ca_path: Option<String>,
    client_auth_mode: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct ParsedUpstreamTls {
    verify: Option<ConvertedUpstreamVerify>,
    versions: Vec<String>,
    client_cert_path: Option<String>,
    client_key_path: Option<String>,
    server_name: Option<bool>,
    server_name_override: Option<String>,
}

#[derive(Debug)]
enum ParsedMatcher {
    Exact(String),
    Prefix(String),
}

impl ParsedNginxConfig {
    fn from_statements(statements: Vec<Statement>) -> Result<Self> {
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
                    parsed.servers.push(parse_server_block(children, &mut parsed.warnings)?);
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
                    self.servers.push(parse_server_block(children, &mut self.warnings)?);
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
                tls.cert_path = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "ssl_certificate_key" => {
                tls.key_path = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "ssl_client_certificate" => {
                tls.client_ca_path = args.first().cloned();
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
            Statement::Block { name, args, children } if name == "location" => {
                locations.push(parse_location(&args, children, warnings)?);
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
            "default_server" | "http2" | "proxy_protocol" | "reuseport" | "deferred" | "bind" => {
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
            }
            "ssl" => ssl = true,
            option if option.contains('=') => {
                warnings.push(format!("ignored nginx `listen` option `{option}` during migration"))
            }
            candidate => address = Some(normalize_listen_address(candidate)?),
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

                if header_name.eq_ignore_ascii_case("x-forwarded-proto") && header_value == "$scheme"
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
                upstream_tls.versions = parse_tls_versions(&args, "proxy_ssl_protocols", warnings);
            }
            Statement::Directive { name, args } if name == "proxy_ssl_name" => {
                upstream_tls.server_name_override = args.first().cloned();
            }
            Statement::Directive { name, args } if name == "proxy_ssl_server_name" => {
                let value = args
                    .first()
                    .ok_or_else(|| anyhow!("proxy_ssl_server_name requires a value"))?;
                upstream_tls.server_name = Some(!matches!(value.as_str(), "off" | "false" | "0"));
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

fn parse_size(raw: &str) -> Result<u64> {
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

fn convert_server_tls(
    tls: &ParsedServerTls,
    warnings: &mut Vec<String>,
) -> Option<ConvertedServerTls> {
    let (Some(cert_path), Some(key_path)) = (tls.cert_path.clone(), tls.key_path.clone()) else {
        if tls.cert_path.is_some()
            || tls.key_path.is_some()
            || !tls.versions.is_empty()
            || tls.client_auth_mode.is_some()
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
        client_ca_path: tls.client_ca_path.clone(),
        client_auth_mode: tls.client_auth_mode.clone(),
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
    if !tls.versions.is_empty() || tls.client_ca_path.is_some() || tls.client_auth_mode.is_some() {
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
struct ConvertedConfig {
    listeners: Vec<ConvertedListener>,
    server: ConvertedServer,
    vhosts: Vec<ConvertedVhost>,
    upstreams: Vec<ConvertedUpstream>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct ConvertedListener {
    name: String,
    listen: String,
    tls: Option<ConvertedServerTls>,
}

#[derive(Debug)]
struct ConvertedServer {
    listen: Option<String>,
    server_names: Vec<String>,
    max_request_body_bytes: Option<u64>,
    tls: Option<ConvertedServerTls>,
    locations: Vec<ConvertedLocation>,
}

#[derive(Debug)]
struct ConvertedVhost {
    server_names: Vec<String>,
    tls: Option<ConvertedVhostTls>,
    locations: Vec<ConvertedLocation>,
}

#[derive(Debug)]
struct ConvertedUpstream {
    name: String,
    peers: Vec<ConvertedPeer>,
    tls: Option<ConvertedUpstreamTls>,
    server_name: Option<bool>,
    server_name_override: Option<String>,
}

#[derive(Debug)]
struct ConvertedPeer {
    url: String,
    weight: u32,
    backup: bool,
}

#[derive(Debug)]
struct ConvertedLocation {
    matcher: ConvertedMatcher,
    upstream_name: String,
    preserve_host: bool,
    proxy_set_headers: BTreeMap<String, String>,
}

#[derive(Debug)]
enum ConvertedMatcher {
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
struct ConvertedServerTls {
    cert_path: String,
    key_path: String,
    versions: Vec<String>,
    client_ca_path: Option<String>,
    client_auth_mode: Option<String>,
}

#[derive(Debug, Clone)]
struct ConvertedVhostTls {
    cert_path: String,
    key_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConvertedUpstreamVerify {
    NativeRoots,
    CustomCa(String),
    Insecure,
}

#[derive(Debug, Clone)]
struct ConvertedUpstreamTls {
    verify: ConvertedUpstreamVerify,
    versions: Vec<String>,
    client_cert_path: Option<String>,
    client_key_path: Option<String>,
}

impl ConvertedConfig {
    fn from_parsed(parsed: ParsedNginxConfig) -> Result<Self> {
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

    fn render(&self, source_label: &str) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "// Generated by `rginx migrate-nginx` from {:?}.", source_label);
        let _ = writeln!(
            out,
            "// Review every warning before running `rginx check` or deploying the result."
        );
        if self.warnings.is_empty() {
            let _ = writeln!(out, "// No lossy migration warnings were recorded.");
        } else {
            let _ = writeln!(out, "// Migration warnings:");
            for warning in &self.warnings {
                let _ = writeln!(out, "// - {}", warning);
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "Config(");
        let _ = writeln!(out, "    runtime: RuntimeConfig(");
        let _ = writeln!(out, "        shutdown_timeout_secs: 30,");
        let _ = writeln!(out, "    ),");

        if !self.listeners.is_empty() {
            let _ = writeln!(out, "    listeners: [");
            for listener in &self.listeners {
                let _ = writeln!(out, "        ListenerConfig(");
                let _ = writeln!(out, "            name: {},", ron_string(&listener.name));
                let _ = writeln!(out, "            listen: {},", ron_string(&listener.listen));
                render_listener_tls(&mut out, "            ", listener.tls.as_ref());
                let _ = writeln!(out, "        ),");
            }
            let _ = writeln!(out, "    ],");
        }

        render_server(&mut out, &self.server);
        render_upstreams(&mut out, &self.upstreams);
        render_locations(&mut out, "    locations", &self.server.locations);
        render_vhosts(&mut out, &self.vhosts);
        let _ = writeln!(out, ")");
        out
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
            let upstream_name = if let Some(upstream) = pending_upstreams.get_mut(&target.authority) {
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
                    let name = implicit_upstream_name(index, &target.authority, implicit_upstream_names.len());
                    implicit_upstream_names.insert(key.clone(), name.clone());
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
                merge_upstream_tls(&mut upstream.tls, &location.upstream_tls, &upstream.name, warnings);
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
struct ProxyPassTarget {
    scheme: String,
    authority: String,
}

fn parse_proxy_pass_target(raw: &str) -> Result<ProxyPassTarget> {
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

fn render_server(out: &mut String, server: &ConvertedServer) {
    let _ = writeln!(out, "    server: ServerConfig(");
    if let Some(listen) = &server.listen {
        let _ = writeln!(out, "        listen: {},", ron_string(listen));
    }
    if !server.server_names.is_empty() {
        let _ = writeln!(out, "        server_names: {},", ron_string_list(&server.server_names));
    }
    if let Some(limit) = server.max_request_body_bytes {
        let _ = writeln!(out, "        max_request_body_bytes: Some({limit}),");
    }
    render_server_tls(out, "        ", server.tls.as_ref());
    let _ = writeln!(out, "    ),");
}

fn render_upstreams(out: &mut String, upstreams: &[ConvertedUpstream]) {
    let _ = writeln!(out, "    upstreams: [");
    for upstream in upstreams {
        let _ = writeln!(out, "        UpstreamConfig(");
        let _ = writeln!(out, "            name: {},", ron_string(&upstream.name));
        let _ = writeln!(out, "            peers: [");
        for peer in &upstream.peers {
            let _ = writeln!(out, "                UpstreamPeerConfig(");
            let _ = writeln!(out, "                    url: {},", ron_string(&peer.url));
            if peer.weight != 1 {
                let _ = writeln!(out, "                    weight: {},", peer.weight);
            }
            if peer.backup {
                let _ = writeln!(out, "                    backup: true,");
            }
            let _ = writeln!(out, "                ),");
        }
        let _ = writeln!(out, "            ],");
        if let Some(tls) = upstream.tls.as_ref() {
            render_upstream_tls(out, "            ", tls);
        }
        if let Some(server_name) = upstream.server_name {
            let _ = writeln!(out, "            server_name: Some({server_name}),");
        }
        if let Some(server_name_override) = upstream.server_name_override.as_ref() {
            let _ = writeln!(
                out,
                "            server_name_override: Some({}),",
                ron_string(server_name_override)
            );
        }
        let _ = writeln!(out, "        ),");
    }
    let _ = writeln!(out, "    ],");
}

fn render_locations(out: &mut String, label: &str, locations: &[ConvertedLocation]) {
    let _ = writeln!(out, "{label}: [");
    for location in locations {
        let _ = writeln!(out, "        LocationConfig(");
        match &location.matcher {
            ConvertedMatcher::Exact(path) => {
                let _ = writeln!(out, "            matcher: Exact({}),", ron_string(path));
            }
            ConvertedMatcher::Prefix(path) => {
                let _ = writeln!(out, "            matcher: Prefix({}),", ron_string(path));
            }
        }
        let _ = writeln!(out, "            handler: Proxy(");
        let _ = writeln!(out, "                upstream: {},", ron_string(&location.upstream_name));
        if location.preserve_host {
            let _ = writeln!(out, "                preserve_host: Some(true),");
        }
        if !location.proxy_set_headers.is_empty() {
            let _ = writeln!(out, "                proxy_set_headers: {{");
            for (name, value) in &location.proxy_set_headers {
                let _ = writeln!(
                    out,
                    "                    {}: {},",
                    ron_string(name),
                    ron_string(value)
                );
            }
            let _ = writeln!(out, "                }},");
        }
        let _ = writeln!(out, "            ),");
        let _ = writeln!(out, "        ),");
    }
    let _ = writeln!(out, "    ],");
}

fn render_vhosts(out: &mut String, vhosts: &[ConvertedVhost]) {
    let _ = writeln!(out, "    servers: [");
    for vhost in vhosts {
        let _ = writeln!(out, "        VirtualHostConfig(");
        let _ =
            writeln!(out, "            server_names: {},", ron_string_list(&vhost.server_names));
        if let Some(tls) = vhost.tls.as_ref() {
            let _ = writeln!(out, "            tls: Some(VirtualHostTlsConfig(");
            let _ = writeln!(out, "                cert_path: {},", ron_string(&tls.cert_path));
            let _ = writeln!(out, "                key_path: {},", ron_string(&tls.key_path));
            let _ = writeln!(out, "            )),");
        }
        render_locations(out, "            locations", &vhost.locations);
        let _ = writeln!(out, "        ),");
    }
    let _ = writeln!(out, "    ],");
}

fn ron_string(value: &str) -> String {
    format!("{value:?}")
}

fn ron_string_list(values: &[String]) -> String {
    let entries = values.iter().map(|value| ron_string(value)).collect::<Vec<_>>().join(", ");
    format!("[{entries}]")
}

fn ron_enum_list(values: &[String]) -> String {
    format!("[{}]", values.join(", "))
}

fn render_listener_tls(out: &mut String, indent: &str, tls: Option<&ConvertedServerTls>) {
    if let Some(tls) = tls {
        let _ = writeln!(out, "{indent}tls: Some(ServerTlsConfig(");
        render_server_tls_body(out, &format!("{indent}    "), tls);
        let _ = writeln!(out, "{indent})),");
    }
}

fn render_server_tls(out: &mut String, indent: &str, tls: Option<&ConvertedServerTls>) {
    if let Some(tls) = tls {
        let _ = writeln!(out, "{indent}tls: Some(ServerTlsConfig(");
        render_server_tls_body(out, &format!("{indent}    "), tls);
        let _ = writeln!(out, "{indent})),");
    }
}

fn render_server_tls_body(out: &mut String, indent: &str, tls: &ConvertedServerTls) {
    let _ = writeln!(out, "{indent}cert_path: {},", ron_string(&tls.cert_path));
    let _ = writeln!(out, "{indent}key_path: {},", ron_string(&tls.key_path));
    if !tls.versions.is_empty() {
        let _ = writeln!(out, "{indent}versions: Some({}),", ron_enum_list(&tls.versions));
    }
    if let (Some(ca_path), Some(mode)) = (&tls.client_ca_path, &tls.client_auth_mode) {
        let _ = writeln!(out, "{indent}client_auth: Some(ServerClientAuthConfig(");
        let _ = writeln!(out, "{indent}    mode: {mode},");
        let _ = writeln!(out, "{indent}    ca_cert_path: {},", ron_string(ca_path));
        let _ = writeln!(out, "{indent})),");
    }
}

fn render_upstream_tls(out: &mut String, indent: &str, tls: &ConvertedUpstreamTls) {
    let shorthand = matches!(tls.verify, ConvertedUpstreamVerify::Insecure)
        && tls.versions.is_empty()
        && tls.client_cert_path.is_none()
        && tls.client_key_path.is_none();
    if shorthand {
        let _ = writeln!(out, "{indent}tls: Some(Insecure),");
        return;
    }

    let _ = writeln!(out, "{indent}tls: Some(UpstreamTlsConfig(");
    match &tls.verify {
        ConvertedUpstreamVerify::NativeRoots => {
            let _ = writeln!(out, "{indent}    verify: NativeRoots,");
        }
        ConvertedUpstreamVerify::Insecure => {
            let _ = writeln!(out, "{indent}    verify: Insecure,");
        }
        ConvertedUpstreamVerify::CustomCa(path) => {
            let _ = writeln!(out, "{indent}    verify: CustomCa(");
            let _ = writeln!(out, "{indent}        ca_cert_path: {},", ron_string(path));
            let _ = writeln!(out, "{indent}    ),");
        }
    }
    if !tls.versions.is_empty() {
        let _ = writeln!(out, "{indent}    versions: Some({}),", ron_enum_list(&tls.versions));
    }
    if let Some(path) = tls.client_cert_path.as_ref() {
        let _ = writeln!(out, "{indent}    client_cert_path: Some({}),", ron_string(path));
    }
    if let Some(path) = tls.client_key_path.as_ref() {
        let _ = writeln!(out, "{indent}    client_key_path: Some({}),", ron_string(path));
    }
    let _ = writeln!(out, "{indent})),");
}

#[cfg(test)]
mod tests {
    use super::{migrate_source, parse_proxy_pass_target, parse_size};

    #[test]
    fn parse_size_supports_nginx_suffixes() {
        assert_eq!(parse_size("8k").unwrap(), 8 * 1024);
        assert_eq!(parse_size("10m").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("1g").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("512").unwrap(), 512);
    }

    #[test]
    fn parse_proxy_pass_rejects_uri_paths() {
        let error = parse_proxy_pass_target("http://backend/api").expect_err("path should fail");
        assert!(error.to_string().contains("contains a URI path"));
    }

    #[test]
    fn migrate_source_renders_supported_subset() {
        let migrated = migrate_source(
            r#"
            worker_processes auto;
            events {}
            http {
                upstream backend {
                    server 10.0.0.10:8080 weight=3;
                    server 10.0.0.11:8080 backup;
                }

                server {
                    listen 8080;
                    server_name api.example.com;
                    client_max_body_size 10m;

                    location = /healthz {
                        proxy_pass http://backend;
                        proxy_set_header Host $host;
                        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
                        proxy_set_header X-Trace-Static static-value;
                    }
                }
            }
            "#,
            "inline.conf",
        )
        .expect("migration should succeed");

        assert!(migrated.ron.contains("listen: \"0.0.0.0:8080\""));
        assert!(migrated.ron.contains("server_names: [\"api.example.com\"]"));
        assert!(migrated.ron.contains("max_request_body_bytes: Some(10485760)"));
        assert!(migrated.ron.contains("upstream: \"backend\""));
        assert!(migrated.ron.contains("preserve_host: Some(true)"));
        assert!(migrated.ron.contains("\"X-Trace-Static\": \"static-value\""));
        assert!(!migrated.ron.contains("X-Forwarded-For"));
    }
}
