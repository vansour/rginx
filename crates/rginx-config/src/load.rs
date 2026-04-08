use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use rginx_core::{Error, Result};

use crate::model::Config;

pub fn load_from_path(path: impl AsRef<Path>) -> Result<Config> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path)?;
    load_from_str(&contents, path)
}

pub fn load_from_str(contents: &str, source_path: impl AsRef<Path>) -> Result<Config> {
    let source_path = source_path.as_ref();
    let expanded = preprocess_source(contents, source_path)?;
    ron::de::from_str(&expanded).map_err(|error| {
        Error::Config(format!("failed to parse {}: {error}", source_path.display()))
    })
}

fn preprocess_source(contents: &str, source_path: &Path) -> Result<String> {
    preprocess_source_inner(contents, source_path, &mut Vec::new())
}

fn preprocess_source_inner(
    contents: &str,
    source_path: &Path,
    include_stack: &mut Vec<PathBuf>,
) -> Result<String> {
    let normalized_source = normalize_path_for_stack(source_path)?;
    if let Some(index) = include_stack.iter().position(|path| path == &normalized_source) {
        let mut cycle = include_stack[index..]
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        cycle.push(normalized_source.display().to_string());
        return Err(Error::Config(format!(
            "config include cycle detected: {}",
            cycle.join(" -> ")
        )));
    }

    include_stack.push(normalized_source);
    let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    let mut expanded = String::with_capacity(contents.len());

    for line in split_lines_preserving_newlines(contents) {
        match parse_include_directive(line, source_path)? {
            Some(include_pattern) => {
                let include_paths = resolve_include_paths(&include_pattern, base_dir, source_path)?;

                for include_path in include_paths {
                    let include_contents = fs::read_to_string(&include_path).map_err(|error| {
                        Error::Config(format!(
                            "failed to read included config `{}` from `{}`: {error}",
                            include_path.display(),
                            source_path.display()
                        ))
                    })?;
                    let included =
                        preprocess_source_inner(&include_contents, &include_path, include_stack)?;
                    expanded.push_str(&included);
                    if line.ends_with('\n') && !included.ends_with('\n') {
                        expanded.push('\n');
                    }
                }
            }
            None => expanded.push_str(&expand_env_placeholders_in_ron_strings(line, source_path)?),
        }
    }

    include_stack.pop();
    Ok(expanded)
}

fn parse_include_directive(line: &str, source_path: &Path) -> Result<Option<String>> {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix("// @include ") else {
        return Ok(None);
    };

    let include_path: String = ron::de::from_str(rest).map_err(|error| {
        Error::Config(format!("invalid include directive in `{}`: {error}", source_path.display()))
    })?;
    if include_path.trim().is_empty() {
        return Err(Error::Config(format!(
            "include directive in `{}` must not be empty",
            source_path.display()
        )));
    }

    Ok(Some(include_path))
}

fn resolve_include_paths(
    include_pattern: &str,
    base_dir: &Path,
    source_path: &Path,
) -> Result<Vec<PathBuf>> {
    let include_path = PathBuf::from(include_pattern);
    let resolved =
        if include_path.is_absolute() { include_path } else { base_dir.join(include_path) };

    if !include_pattern.contains('*') {
        return Ok(vec![resolved]);
    }

    expand_simple_glob(&resolved, source_path)
}

fn expand_simple_glob(pattern: &Path, source_path: &Path) -> Result<Vec<PathBuf>> {
    let Some(file_name) = pattern.file_name().and_then(|value| value.to_str()) else {
        return Err(Error::Config(format!(
            "invalid include glob `{}` in `{}`",
            pattern.display(),
            source_path.display()
        )));
    };

    if file_name != "*.ron"
        || pattern.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|segment| segment.contains('*') && segment != "*.ron")
        })
    {
        return Err(Error::Config(format!(
            "unsupported include glob `{}` in `{}`; only `*.ron` file globs are supported",
            pattern.display(),
            source_path.display()
        )));
    }

    let parent = pattern.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Ok(Vec::new());
    }
    if !parent.is_dir() {
        return Err(Error::Config(format!(
            "include glob parent `{}` from `{}` is not a directory",
            parent.display(),
            source_path.display()
        )));
    }

    let mut matches = fs::read_dir(parent)
        .map_err(|error| {
            Error::Config(format!(
                "failed to read include directory `{}` from `{}`: {error}",
                parent.display(),
                source_path.display()
            ))
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("ron"))
        .collect::<Vec<_>>();
    matches.sort();
    Ok(matches)
}

fn split_lines_preserving_newlines(contents: &str) -> Vec<&str> {
    if contents.is_empty() {
        return vec![contents];
    }

    let mut lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    if !contents.ends_with('\n') {
        if let Some(last) = contents.rsplit_once('\n').map(|(_, tail)| tail) {
            if lines.last().copied() != Some(last) {
                lines.push(last);
            }
        }
    }
    lines
}

fn expand_env_placeholders_in_ron_strings(contents: &str, source_path: &Path) -> Result<String> {
    let chars = contents.chars().collect::<Vec<_>>();
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut expanded = String::with_capacity(contents.len());

    while let Some(&ch) = chars.get(index) {
        if !in_string {
            expanded.push(ch);
            if ch == '"' {
                in_string = true;
            }
            index += 1;
            continue;
        }

        if escaped {
            expanded.push(ch);
            escaped = false;
            index += 1;
            continue;
        }

        match ch {
            '\\' => {
                expanded.push(ch);
                escaped = true;
                index += 1;
            }
            '"' => {
                expanded.push(ch);
                in_string = false;
                index += 1;
            }
            '$' if chars.get(index + 1) == Some(&'$') => {
                expanded.push('$');
                index += 2;
            }
            '$' if chars.get(index + 1) == Some(&'{') => {
                let end = chars[index + 2..]
                    .iter()
                    .position(|candidate| *candidate == '}')
                    .map(|offset| index + 2 + offset)
                    .ok_or_else(|| {
                        Error::Config(format!(
                            "unterminated environment placeholder in `{}`",
                            source_path.display()
                        ))
                    })?;
                let token = chars[index + 2..end].iter().collect::<String>();
                let replacement = resolve_env_placeholder(&token, source_path)?;
                expanded.push_str(&escape_ron_string_fragment(&replacement));
                index = end + 1;
            }
            _ => {
                expanded.push(ch);
                index += 1;
            }
        }
    }

    Ok(expanded)
}

fn resolve_env_placeholder(token: &str, source_path: &Path) -> Result<String> {
    let (name, default) = match token.split_once(":-") {
        Some((name, default)) => (name, Some(default)),
        None => (token, None),
    };
    if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(Error::Config(format!(
            "invalid environment placeholder `${{{token}}}` in `{}`",
            source_path.display()
        )));
    }

    match env::var(name) {
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => default.map(str::to_string).ok_or_else(|| {
            Error::Config(format!(
                "environment variable `{name}` is not set while loading `{}`",
                source_path.display()
            ))
        }),
        Err(env::VarError::NotUnicode(_)) => Err(Error::Config(format!(
            "environment variable `{name}` is not valid UTF-8 while loading `{}`",
            source_path.display()
        ))),
    }
}

fn escape_ron_string_fragment(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\0' => escaped.push_str("\\0"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn normalize_path_for_stack(path: &Path) -> Result<PathBuf> {
    let absolute =
        if path.is_absolute() { path.to_path_buf() } else { env::current_dir()?.join(path) };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{load_from_path, load_from_str};

    #[test]
    fn load_from_str_expands_environment_placeholders_inside_strings() {
        let _guard = env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("rginx_test_listen", "127.0.0.1:19090");
            std::env::set_var("rginx_test_body", "hello \"env\"\n");
        }

        let config = load_from_str(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"${rginx_test_listen}\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"${rginx_test_body}\"),\n            ),\n        ),\n    ],\n)\n",
            Path::new("inline.ron"),
        )
        .expect("config should load with env expansion");

        assert_eq!(config.server.listen.as_deref(), Some("127.0.0.1:19090"));
        match &config.locations[0].handler {
            crate::model::HandlerConfig::Return { body, .. } => {
                assert_eq!(body.as_deref(), Some("hello \"env\"\n"));
            }
            _ => panic!("expected return handler"),
        }

        unsafe {
            std::env::remove_var("rginx_test_listen");
            std::env::remove_var("rginx_test_body");
        }
    }

    #[test]
    fn load_from_str_supports_env_defaults_and_literal_dollar_escape() {
        let _guard = env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::remove_var("rginx_test_missing");
        }

        let config = load_from_str(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"${rginx_test_missing:-127.0.0.1:18080}\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"$${rginx_test_missing}\"),\n            ),\n        ),\n    ],\n)\n",
            Path::new("inline.ron"),
        )
        .expect("config should load with env defaults");

        assert_eq!(config.server.listen.as_deref(), Some("127.0.0.1:18080"));
        match &config.locations[0].handler {
            crate::model::HandlerConfig::Return { body, .. } => {
                assert_eq!(body.as_deref(), Some("${rginx_test_missing}"));
            }
            _ => panic!("expected return handler"),
        }
    }

    #[test]
    fn load_from_str_supports_legacy_and_structured_upstream_tls_config() {
        let config = load_from_str(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:18080\",\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"legacy\",\n            peers: [UpstreamPeerConfig(url: \"https://legacy.example.com\")],\n            tls: Some(Insecure),\n        ),\n        UpstreamConfig(\n            name: \"structured\",\n            peers: [UpstreamPeerConfig(url: \"https://structured.example.com\")],\n            tls: Some(UpstreamTlsConfig(\n                verify: CustomCa(ca_cert_path: \"ca.pem\"),\n                versions: Some([Tls13]),\n                client_cert_path: Some(\"client.crt\"),\n                client_key_path: Some(\"client.key\"),\n            )),\n        ),\n    ],\n    locations: [\n        LocationConfig(\n            matcher: Prefix(\"/\"),\n            handler: Proxy(upstream: \"legacy\"),\n        ),\n    ],\n)\n",
            Path::new("inline.ron"),
        )
        .expect("TLS config variants should deserialize");

        let legacy = config.upstreams[0].tls.as_ref().expect("legacy TLS should exist");
        assert!(matches!(legacy.verify, crate::model::UpstreamTlsModeConfig::Insecure));
        assert!(legacy.versions.is_none());

        let structured = config.upstreams[1].tls.as_ref().expect("structured TLS should exist");
        assert_eq!(structured.client_cert_path.as_deref(), Some("client.crt"));
        assert_eq!(structured.client_key_path.as_deref(), Some("client.key"));
        assert!(matches!(
            structured.versions.as_deref(),
            Some([crate::model::TlsVersionConfig::Tls13])
        ));
        assert!(matches!(
            structured.verify,
            crate::model::UpstreamTlsModeConfig::CustomCa { .. }
        ));
    }

    #[test]
    fn load_from_str_rejects_missing_environment_placeholders() {
        let _guard = env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::remove_var("rginx_test_required");
        }

        let error = load_from_str(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"${rginx_test_required}\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
            Path::new("inline.ron"),
        )
        .expect_err("missing env placeholder should fail");

        assert!(
            error.to_string().contains("environment variable `rginx_test_required` is not set")
        );
    }

    #[test]
    fn load_from_path_expands_relative_includes_recursively() {
        let temp_dir = temp_dir("rginx-load-include-test");
        fs::create_dir_all(temp_dir.join("fragments"))
            .expect("temp fragments dir should be created");
        let config_path = temp_dir.join("rginx.ron");
        let routes_path = temp_dir.join("fragments/routes.ron");
        let body_path = temp_dir.join("fragments/body.ron");

        fs::write(&body_path, "\"included body\\n\"").expect("body fragment should be written");
        fs::write(
            &routes_path,
            "LocationConfig(\n    matcher: Exact(\"/\"),\n    handler: Return(\n        status: 200,\n        location: \"\",\n        body: Some(\n            // @include \"body.ron\"\n        ),\n    ),\n),\n",
        )
        .expect("routes fragment should be written");
        fs::write(
            &config_path,
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:18081\",\n    ),\n    upstreams: [],\n    locations: [\n        // @include \"fragments/routes.ron\"\n    ],\n)\n",
        )
        .expect("root config should be written");

        let config = load_from_path(&config_path).expect("config with includes should load");
        match &config.locations[0].handler {
            crate::model::HandlerConfig::Return { body, .. } => {
                assert_eq!(body.as_deref(), Some("included body\n"));
            }
            _ => panic!("expected return handler"),
        }

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn load_from_path_rejects_include_cycles() {
        let temp_dir = temp_dir("rginx-load-include-cycle-test");
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let first = temp_dir.join("first.ron");
        let second = temp_dir.join("second.ron");

        fs::write(&first, "// @include \"second.ron\"\n").expect("first include should be written");
        fs::write(&second, "// @include \"first.ron\"\n")
            .expect("second include should be written");

        let error = load_from_path(&first).expect_err("include cycle should fail");
        assert!(error.to_string().contains("config include cycle detected"));

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn load_from_path_expands_sorted_conf_d_glob_fragments() {
        let temp_dir = temp_dir("rginx-load-conf-d-test");
        let conf_d = temp_dir.join("conf.d");
        fs::create_dir_all(&conf_d).expect("conf.d should be created");
        let config_path = temp_dir.join("rginx.ron");

        fs::write(
            conf_d.join("20-app.ron"),
            "VirtualHostConfig(\n    server_names: [\"app.example.com\"],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"app\\n\"),\n            ),\n        ),\n    ],\n),\n",
        )
        .expect("app vhost should be written");
        fs::write(
            conf_d.join("10-api.ron"),
            "VirtualHostConfig(\n    server_names: [\"api.example.com\"],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"api\\n\"),\n            ),\n        ),\n    ],\n),\n",
        )
        .expect("api vhost should be written");
        fs::write(
            &config_path,
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"0.0.0.0:80\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"root\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        // @include \"conf.d/*.ron\"\n    ],\n)\n",
        )
        .expect("root config should be written");

        let config = load_from_path(&config_path).expect("config with conf.d glob should load");

        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers[0].server_names, vec!["api.example.com".to_string()]);
        assert_eq!(config.servers[1].server_names, vec!["app.example.com".to_string()]);

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn load_from_path_rejects_unsupported_include_globs() {
        let error = load_from_str(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"0.0.0.0:80\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        // @include \"conf.d/*.txt\"\n    ],\n)\n",
            Path::new("rginx.ron"),
        )
        .expect_err("unsupported glob should fail");

        assert!(error.to_string().contains("only `*.ron` file globs are supported"));
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{unique}"))
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
}
