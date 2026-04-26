use std::fs;
use std::path::{Component, Path, PathBuf};

use rginx_core::{Error, Result};

pub(super) fn parse_include_directive(line: &str, source_path: &Path) -> Result<Option<String>> {
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

pub(super) fn resolve_include_paths(
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

pub(super) fn split_lines_preserving_newlines(contents: &str) -> Vec<&str> {
    if contents.is_empty() {
        return vec![contents];
    }

    let mut lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    if !contents.ends_with('\n')
        && let Some(last) = contents.rsplit_once('\n').map(|(_, tail)| tail)
        && lines.last().copied() != Some(last)
    {
        lines.push(last);
    }
    lines
}

pub(super) fn normalize_path_for_stack(path: &Path) -> Result<PathBuf> {
    let normalized = if path.is_absolute() {
        path.components().fold(PathBuf::new(), normalize_component)
    } else {
        let cwd = std::env::current_dir().map_err(|error| {
            Error::Config(format!(
                "failed to resolve relative config path `{}`: {error}",
                path.display()
            ))
        })?;
        cwd.join(path).components().fold(PathBuf::new(), normalize_component)
    };

    Ok(normalized)
}

fn normalize_component(mut current: PathBuf, component: Component<'_>) -> PathBuf {
    match component {
        Component::CurDir => current,
        Component::ParentDir => {
            current.pop();
            current
        }
        other => {
            current.push(other.as_os_str());
            current
        }
    }
}
