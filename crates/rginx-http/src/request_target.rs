use http::Uri;

pub(crate) struct NormalizedRequestTarget {
    pub(crate) path: String,
    pub(crate) path_and_query: String,
}

pub(crate) fn normalize_request_target(uri: &Uri) -> NormalizedRequestTarget {
    let path = normalize_request_path(uri.path());
    let path_and_query = match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.clone(),
    };

    NormalizedRequestTarget { path, path_and_query }
}

pub(crate) fn normalize_request_path(path: &str) -> String {
    let path = if path.is_empty() { "/" } else { path };
    let absolute = path.starts_with('/');
    let trailing_slash = path.ends_with('/');
    let mut normalized_segments = Vec::new();

    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                normalized_segments.pop();
            }
            _ => normalized_segments.push(segment),
        }
    }

    let mut normalized = String::new();
    if absolute {
        normalized.push('/');
    }

    normalized.push_str(&normalized_segments.join("/"));

    if normalized.is_empty() {
        normalized.push('/');
    }

    if trailing_slash && normalized != "/" && !normalized.ends_with('/') {
        normalized.push('/');
    }

    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }

    normalized
}
