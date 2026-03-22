use std::path::{Path, PathBuf};

use http::StatusCode;
use hyper::body::Incoming;
use hyper::{Request, Response};

use rginx_core::FileTarget;

use crate::handler::{full_body, text_response, HttpResponse};

pub async fn serve_file(request: Request<Incoming>, target: &FileTarget) -> HttpResponse {
    let uri = request.uri();
    let request_path = uri.path();

    // Decode percent-encoded path
    let decoded_path = match percent_encoding::percent_decode_str(request_path).decode_utf8() {
        Ok(path) => path,
        Err(_) => {
            return text_response(
                StatusCode::BAD_REQUEST,
                "text/plain; charset=utf-8",
                "invalid path encoding\n",
            );
        }
    };

    // Security check: prevent directory traversal
    if !is_path_safe(&target.root, &decoded_path) {
        tracing::warn!(
            path = %request_path,
            root = %target.root.display(),
            "file access blocked: path traversal attempt"
        );
        return text_response(StatusCode::FORBIDDEN, "text/plain; charset=utf-8", "forbidden\n");
    }

    // Try to resolve the file path
    let file_path = resolve_file_path(&target.root, &decoded_path);

    // Handle try_files logic
    let resolved_path = if !file_path.exists() {
        // File doesn't exist, try try_files fallbacks
        resolve_try_files(&target.root, &decoded_path, &target.try_files)
    } else if file_path.is_dir() {
        // It's a directory, try index file
        if let Some(ref index) = target.index {
            let index_path = file_path.join(index);
            if index_path.exists() && index_path.is_file() {
                Some(index_path)
            } else {
                // Try try_files for directory
                resolve_try_files(&target.root, &decoded_path, &target.try_files)
            }
        } else {
            resolve_try_files(&target.root, &decoded_path, &target.try_files)
        }
    } else {
        Some(file_path)
    };

    let Some(file_path) = resolved_path else {
        return text_response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", "not found\n");
    };

    // Read file
    match tokio::fs::read(&file_path).await {
        Ok(content) => {
            let content_type = detect_content_type(&file_path);
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", content_type)
                .body(full_body(content))
                .expect("response builder should not fail for file responses")
        }
        Err(err) => {
            tracing::error!(
                path = %file_path.display(),
                error = %err,
                "failed to read file"
            );
            text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "text/plain; charset=utf-8",
                "internal server error\n",
            )
        }
    }
}

fn resolve_file_path(root: &Path, request_path: &str) -> PathBuf {
    // Remove leading slash and join to root
    let relative_path = request_path.strip_prefix('/').unwrap_or(request_path);
    root.join(relative_path)
}

fn resolve_try_files(root: &Path, request_path: &str, try_files: &[String]) -> Option<PathBuf> {
    for candidate in try_files {
        let resolved = if candidate == "$uri" {
            // Try the request path as a file
            resolve_file_path(root, request_path)
        } else if candidate == "$uri/" {
            // Try the request path as a directory with index
            let dir_path = resolve_file_path(root, request_path);
            dir_path.join("index.html")
        } else if candidate.starts_with('/') {
            // Absolute path fallback (e.g., /index.html for SPA)
            resolve_file_path(root, candidate)
        } else {
            // Relative path
            let base = resolve_file_path(root, request_path);
            base.parent().map(|p| p.join(candidate)).unwrap_or_else(|| base.clone())
        };

        if resolved.exists() {
            // Security check for the resolved path
            if is_path_safe(root, &resolved.to_string_lossy()) {
                return Some(resolved);
            }
        }
    }
    None
}

fn is_path_safe(root: &Path, request_path: &str) -> bool {
    // Check for path traversal attempts
    if request_path.contains("..") {
        return false;
    }

    // Resolve the path and check if it's under root
    let resolved = resolve_file_path(root, request_path);

    // Canonicalize both paths for comparison
    // Note: We don't require the file to exist for the safety check
    let normalized = normalize_path(&resolved);

    // Check if the normalized path starts with root
    normalized.starts_with(root) || is_subpath(root, &normalized)
}

fn is_subpath(parent: &Path, child: &Path) -> bool {
    // Compare path components
    let parent_components: Vec<_> = parent.components().collect();
    let child_components: Vec<_> = child.components().collect();

    if child_components.len() < parent_components.len() {
        return false;
    }

    parent_components.iter().zip(child_components.iter()).all(|(p, c)| p == c)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    // Path tries to go above root
                    return PathBuf::new();
                }
            }
            _ => normalized.push(component),
        }
    }
    normalized
}

fn detect_content_type(path: &Path) -> String {
    let extension = path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_lowercase());

    match extension.as_deref() {
        Some("html") | Some("htm") => "text/html; charset=utf-8".to_string(),
        Some("css") => "text/css; charset=utf-8".to_string(),
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8".to_string(),
        Some("json") => "application/json; charset=utf-8".to_string(),
        Some("xml") => "application/xml; charset=utf-8".to_string(),
        Some("txt") => "text/plain; charset=utf-8".to_string(),
        Some("csv") => "text/csv; charset=utf-8".to_string(),
        Some("png") => "image/png".to_string(),
        Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("svg") => "image/svg+xml".to_string(),
        Some("ico") => "image/x-icon".to_string(),
        Some("webp") => "image/webp".to_string(),
        Some("woff") => "font/woff".to_string(),
        Some("woff2") => "font/woff2".to_string(),
        Some("ttf") => "font/ttf".to_string(),
        Some("otf") => "font/otf".to_string(),
        Some("eot") => "application/vnd.ms-fontobject".to_string(),
        Some("pdf") => "application/pdf".to_string(),
        Some("zip") => "application/zip".to_string(),
        Some("gz") | Some("gzip") => "application/gzip".to_string(),
        Some("tar") => "application/x-tar".to_string(),
        Some("mp3") => "audio/mpeg".to_string(),
        Some("mp4") => "video/mp4".to_string(),
        Some("webm") => "video/webm".to_string(),
        Some("weba") => "audio/webm".to_string(),
        Some("wasm") => "application/wasm".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_path_safe_rejects_traversal() {
        assert!(!is_path_safe(Path::new("/var/www"), "../../../etc/passwd"));
        assert!(!is_path_safe(Path::new("/var/www"), "/static/../etc/passwd"));
    }

    #[test]
    fn is_path_safe_accepts_valid_paths() {
        assert!(is_path_safe(Path::new("/var/www"), "/index.html"));
        assert!(is_path_safe(Path::new("/var/www"), "/static/app.js"));
    }

    #[test]
    fn detect_content_type_handles_common_extensions() {
        assert_eq!(detect_content_type(Path::new("index.html")), "text/html; charset=utf-8");
        assert_eq!(
            detect_content_type(Path::new("app.js")),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(detect_content_type(Path::new("logo.png")), "image/png");
        assert_eq!(detect_content_type(Path::new("unknown.xyz")), "application/octet-stream");
    }
}
