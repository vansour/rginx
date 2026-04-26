use std::env;
use std::path::Path;

use rginx_core::{Error, Result};

pub(super) fn expand_env_placeholders_in_ron_strings(
    contents: &str,
    source_path: &Path,
) -> Result<String> {
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
            _ => escaped.push(ch),
        }
    }
    escaped
}
