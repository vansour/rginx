use std::fs;
use std::path::{Path, PathBuf};

use rginx_core::{Error, Result};

use super::env_expand::expand_env_placeholders_in_ron_strings;
use super::include::{
    normalize_path_for_stack, parse_include_directive, resolve_include_paths,
    split_lines_preserving_newlines,
};

pub(super) fn preprocess_source(contents: &str, source_path: &Path) -> Result<String> {
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
