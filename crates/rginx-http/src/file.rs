use std::path::{Path, PathBuf};

use futures_util::TryStreamExt;
use http::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::{Request, Response};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

use rginx_core::FileTarget;

use crate::handler::{BoxError, HttpBody, HttpResponse, full_body, text_response};

const FILE_STREAM_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

pub async fn serve_file(request: Request<Incoming>, target: &FileTarget) -> HttpResponse {
    let request_path = request.uri().path();
    let head_only = request.method() == Method::HEAD;

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

    if !is_path_safe(&target.root, &decoded_path) {
        tracing::warn!(
            path = %request_path,
            root = %target.root.display(),
            "file access blocked: path traversal attempt"
        );
        return text_response(StatusCode::FORBIDDEN, "text/plain; charset=utf-8", "forbidden\n");
    }

    let Some(file_path) = resolve_file_target(&target.root, &decoded_path, target) else {
        return text_response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", "not found\n");
    };

    let metadata = match tokio::fs::metadata(&file_path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            return text_response(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                "not found\n",
            );
        }
        Err(err) => {
            tracing::error!(
                path = %file_path.display(),
                error = %err,
                "failed to read file metadata"
            );
            return text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "text/plain; charset=utf-8",
                "internal server error\n",
            );
        }
    };

    let file_len = metadata.len();
    let range = match parse_range_header(request.headers(), file_len) {
        Ok(range) => range,
        Err(()) => return range_not_satisfiable(file_len),
    };

    let content_type = detect_content_type(&file_path);
    let mut builder =
        Response::builder().header("content-type", content_type).header("accept-ranges", "bytes");

    match range {
        Some(range) => {
            builder = builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(CONTENT_RANGE, format!("bytes {}-{}/{}", range.start, range.end, file_len))
                .header(CONTENT_LENGTH, range.len().to_string());

            if head_only {
                return builder
                    .body(full_body(Vec::new()))
                    .expect("response builder should not fail for HEAD file responses");
            }

            let mut file = match File::open(&file_path).await {
                Ok(file) => file,
                Err(err) => {
                    tracing::error!(
                        path = %file_path.display(),
                        error = %err,
                        "failed to open file"
                    );
                    return text_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "text/plain; charset=utf-8",
                        "internal server error\n",
                    );
                }
            };
            if let Err(err) = file.seek(SeekFrom::Start(range.start)).await {
                tracing::error!(
                    path = %file_path.display(),
                    error = %err,
                    "failed to seek file"
                );
                return text_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "text/plain; charset=utf-8",
                    "internal server error\n",
                );
            }

            builder
                .body(stream_file_body(file.take(range.len())))
                .expect("response builder should not fail for partial file responses")
        }
        None => {
            builder = builder.status(StatusCode::OK).header(CONTENT_LENGTH, file_len.to_string());

            if head_only {
                return builder
                    .body(full_body(Vec::new()))
                    .expect("response builder should not fail for HEAD file responses");
            }

            let file = match File::open(&file_path).await {
                Ok(file) => file,
                Err(err) => {
                    tracing::error!(
                        path = %file_path.display(),
                        error = %err,
                        "failed to open file"
                    );
                    return text_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "text/plain; charset=utf-8",
                        "internal server error\n",
                    );
                }
            };

            builder
                .body(stream_file_body(file))
                .expect("response builder should not fail for file responses")
        }
    }
}

fn resolve_file_target(root: &Path, request_path: &str, target: &FileTarget) -> Option<PathBuf> {
    let file_path = resolve_file_path(root, request_path);

    if !file_path.exists() {
        return resolve_try_files(root, request_path, &target.try_files);
    }

    if file_path.is_dir() {
        if let Some(ref index) = target.index {
            let index_path = file_path.join(index);
            if index_path.exists() && index_path.is_file() {
                return Some(index_path);
            }
        }
        return resolve_try_files(root, request_path, &target.try_files);
    }

    file_path.is_file().then_some(file_path)
}

fn stream_file_body<R>(reader: R) -> HttpBody
where
    R: tokio::io::AsyncRead + Send + 'static,
{
    StreamBody::new(
        ReaderStream::with_capacity(reader, FILE_STREAM_CHUNK_SIZE)
            .map_ok(Frame::data)
            .map_err(|error| -> BoxError { Box::new(error) }),
    )
    .boxed_unsync()
}

fn range_not_satisfiable(file_len: u64) -> HttpResponse {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(CONTENT_RANGE, format!("bytes */{file_len}"))
        .header(CONTENT_LENGTH, "0")
        .body(full_body(Vec::new()))
        .expect("response builder should not fail for range errors")
}

fn parse_range_header(headers: &HeaderMap, file_len: u64) -> Result<Option<ByteRange>, ()> {
    let Some(value) = headers.get(RANGE) else {
        return Ok(None);
    };

    let value = value.to_str().map_err(|_| ())?.trim();
    let Some(spec) = value.strip_prefix("bytes=") else {
        return Err(());
    };

    if spec.contains(',') {
        return Ok(None);
    }

    let (start, end) = spec.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix_len = end.parse::<u64>().map_err(|_| ())?;
        if suffix_len == 0 {
            return Err(());
        }

        let bounded = suffix_len.min(file_len);
        if bounded == 0 {
            return Err(());
        }

        return Ok(Some(ByteRange { start: file_len - bounded, end: file_len.saturating_sub(1) }));
    }

    let start = start.parse::<u64>().map_err(|_| ())?;
    if start >= file_len {
        return Err(());
    }

    let end = if end.is_empty() {
        file_len.saturating_sub(1)
    } else {
        end.parse::<u64>().map_err(|_| ())?
    };

    if start > end {
        return Err(());
    }

    Ok(Some(ByteRange { start, end: end.min(file_len.saturating_sub(1)) }))
}

fn resolve_file_path(root: &Path, request_path: &str) -> PathBuf {
    let relative_path = request_path.strip_prefix('/').unwrap_or(request_path);
    root.join(relative_path)
}

fn resolve_try_files(root: &Path, request_path: &str, try_files: &[String]) -> Option<PathBuf> {
    for candidate in try_files {
        let resolved = if candidate == "$uri" {
            resolve_file_path(root, request_path)
        } else if candidate == "$uri/" {
            let dir_path = resolve_file_path(root, request_path);
            dir_path.join("index.html")
        } else if candidate.starts_with('/') {
            resolve_file_path(root, candidate)
        } else {
            let base = resolve_file_path(root, request_path);
            base.parent().map(|p| p.join(candidate)).unwrap_or_else(|| base.clone())
        };

        if resolved.exists()
            && is_path_safe(root, &resolved.to_string_lossy())
            && resolved.is_file()
        {
            return Some(resolved);
        }
    }
    None
}

fn is_path_safe(root: &Path, request_path: &str) -> bool {
    if request_path.contains("..") {
        return false;
    }

    let resolved = resolve_file_path(root, request_path);
    let normalized = normalize_path(&resolved);

    normalized.starts_with(root) || is_subpath(root, &normalized)
}

fn is_subpath(parent: &Path, child: &Path) -> bool {
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
    use http::HeaderValue;

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

    #[test]
    fn parse_range_header_supports_standard_byte_ranges() {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=10-19"));

        let range = parse_range_header(&headers, 100).expect("range should parse");
        assert_eq!(range, Some(ByteRange { start: 10, end: 19 }));
    }

    #[test]
    fn parse_range_header_supports_suffix_ranges() {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=-10"));

        let range = parse_range_header(&headers, 100).expect("suffix range should parse");
        assert_eq!(range, Some(ByteRange { start: 90, end: 99 }));
    }

    #[test]
    fn parse_range_header_rejects_unsatisfiable_ranges() {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=100-150"));

        assert!(parse_range_header(&headers, 100).is_err());
    }

    #[test]
    fn parse_range_header_ignores_multi_ranges() {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-9,20-29"));

        let range = parse_range_header(&headers, 100).expect("multi-range fallback should parse");
        assert_eq!(range, None);
    }
}
