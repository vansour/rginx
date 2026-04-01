use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use futures_util::TryStreamExt;
use http::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::{Request, Response};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

use rginx_core::FileTarget;

use crate::handler::{BoxError, HttpBody, HttpResponse, full_body, text_response};

const FILE_STREAM_CHUNK_SIZE: usize = 64 * 1024;
const DIRECTORY_LISTING_CONTENT_TYPE: &str = "text/html; charset=utf-8";
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryListingEntry {
    name: String,
    is_dir: bool,
}

#[derive(Debug)]
enum ResolvedFileTarget {
    File(PathBuf),
    DirectoryListing(PathBuf),
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

    if !is_request_path_safe(&decoded_path) {
        tracing::warn!(
            path = %request_path,
            root = %target.root.display(),
            "file access blocked: path traversal attempt"
        );
        return text_response(StatusCode::FORBIDDEN, "text/plain; charset=utf-8", "forbidden\n");
    }

    let Some(resolved_target) = resolve_file_target(&target.root, &decoded_path, target) else {
        return text_response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", "not found\n");
    };

    let file_path = match resolved_target {
        ResolvedFileTarget::File(file_path) => file_path,
        ResolvedFileTarget::DirectoryListing(directory_path) => {
            return match build_directory_listing_response(
                &directory_path,
                decoded_path.as_ref(),
                head_only,
            )
            .await
            {
                Ok(response) => response,
                Err(err) => {
                    tracing::error!(
                        path = %directory_path.display(),
                        error = %err,
                        "failed to render directory listing"
                    );
                    text_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "text/plain; charset=utf-8",
                        "internal server error\n",
                    )
                }
            };
        }
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

async fn build_directory_listing_response(
    directory_path: &Path,
    request_path: &str,
    head_only: bool,
) -> Result<HttpResponse, std::io::Error> {
    let entries = read_directory_listing(directory_path).await?;
    let html = render_directory_listing_html(request_path, &entries);
    let content_length = html.len().to_string();
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", DIRECTORY_LISTING_CONTENT_TYPE)
        .header(CONTENT_LENGTH, content_length);

    if head_only {
        Ok(builder
            .body(full_body(Vec::new()))
            .expect("response builder should not fail for HEAD directory listings"))
    } else {
        Ok(builder
            .body(full_body(html.into_bytes()))
            .expect("response builder should not fail for directory listings"))
    }
}

async fn read_directory_listing(
    directory_path: &Path,
) -> Result<Vec<DirectoryListingEntry>, std::io::Error> {
    let mut read_dir = tokio::fs::read_dir(directory_path).await?;
    let mut entries = Vec::new();

    while let Some(entry) = read_dir.next_entry().await? {
        let file_type = entry.file_type().await?;
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push(DirectoryListingEntry { name, is_dir: file_type.is_dir() });
    }

    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

fn render_directory_listing_html(request_path: &str, entries: &[DirectoryListingEntry]) -> String {
    let directory_path = normalize_directory_request_path(request_path);
    let title = format!("Index of {directory_path}");
    let mut sorted_entries = entries.iter().collect::<Vec<_>>();
    sorted_entries.sort_by(|left, right| left.name.cmp(&right.name));
    let mut html = String::new();

    html.push_str("<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n");
    writeln!(html, "<title>{}</title>", escape_html(&title))
        .expect("writing directory listing HTML should not fail");
    html.push_str("</head>\n<body>\n");
    writeln!(html, "<h1>{}</h1>", escape_html(&title))
        .expect("writing directory listing HTML should not fail");
    html.push_str("<ul>\n");

    if directory_path != "/" {
        writeln!(
            html,
            "<li><a href=\"{}\">../</a></li>",
            escape_html(&encode_href_path(&parent_directory_path(&directory_path)))
        )
        .expect("writing directory listing HTML should not fail");
    }

    for entry in sorted_entries {
        let display_name =
            if entry.is_dir { format!("{}/", entry.name) } else { entry.name.clone() };
        let href = if directory_path == "/" {
            format!("/{}{suffix}", entry.name, suffix = if entry.is_dir { "/" } else { "" })
        } else {
            format!(
                "{}{}{suffix}",
                directory_path,
                entry.name,
                suffix = if entry.is_dir { "/" } else { "" }
            )
        };
        writeln!(
            html,
            "<li><a href=\"{}\">{}</a></li>",
            escape_html(&encode_href_path(&href)),
            escape_html(&display_name)
        )
        .expect("writing directory listing HTML should not fail");
    }

    html.push_str("</ul>\n</body>\n</html>\n");
    html
}

fn normalize_directory_request_path(request_path: &str) -> String {
    let mut normalized = if request_path.is_empty() {
        "/".to_string()
    } else if request_path.starts_with('/') {
        request_path.to_string()
    } else {
        format!("/{request_path}")
    };

    if normalized != "/" && !normalized.ends_with('/') {
        normalized.push('/');
    }

    normalized
}

fn parent_directory_path(request_path: &str) -> String {
    let normalized = normalize_directory_request_path(request_path);
    if normalized == "/" {
        return normalized;
    }

    let trimmed = normalized.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => format!("{}/", &trimmed[..index]),
    }
}

fn encode_href_path(path: &str) -> String {
    let trailing_slash = path.ends_with('/') && path != "/";
    let segments: Vec<String> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| utf8_percent_encode(segment, PATH_SEGMENT_ENCODE_SET).to_string())
        .collect();

    if segments.is_empty() {
        return "/".to_string();
    }

    let mut encoded = format!("/{}", segments.join("/"));
    if trailing_slash {
        encoded.push('/');
    }
    encoded
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn resolve_file_target(
    root: &Path,
    request_path: &str,
    target: &FileTarget,
) -> Option<ResolvedFileTarget> {
    let canonical_root = canonical_root(root)?;
    let file_path = resolve_file_path(root, request_path);

    if !file_path.exists() {
        return resolve_try_files(root, &canonical_root, request_path, &target.try_files)
            .map(ResolvedFileTarget::File);
    }

    if file_path.is_dir() {
        let directory_path = resolve_existing_path_within_root(&canonical_root, &file_path)?;
        if let Some(ref index) = target.index {
            let index_path = file_path.join(index);
            if let Some(index_path) =
                resolve_existing_file_within_root(&canonical_root, &index_path)
            {
                return Some(ResolvedFileTarget::File(index_path));
            }
        }
        if let Some(resolved) =
            resolve_try_files(root, &canonical_root, request_path, &target.try_files)
        {
            return Some(ResolvedFileTarget::File(resolved));
        }
        if target.autoindex {
            return Some(ResolvedFileTarget::DirectoryListing(directory_path));
        }
        return None;
    }

    resolve_existing_file_within_root(&canonical_root, &file_path).map(ResolvedFileTarget::File)
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

fn resolve_try_files(
    root: &Path,
    canonical_root: &Path,
    request_path: &str,
    try_files: &[String],
) -> Option<PathBuf> {
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

        if let Some(resolved) = resolve_existing_file_within_root(canonical_root, &resolved) {
            return Some(resolved);
        }
    }
    None
}

fn is_request_path_safe(request_path: &str) -> bool {
    !Path::new(request_path).components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        )
    })
}

fn canonical_root(root: &Path) -> Option<PathBuf> {
    root.canonicalize().ok().filter(|path| path.is_dir())
}

fn resolve_existing_file_within_root(canonical_root: &Path, path: &Path) -> Option<PathBuf> {
    let resolved = resolve_existing_path_within_root(canonical_root, path)?;
    resolved.is_file().then_some(resolved)
}

fn resolve_existing_path_within_root(canonical_root: &Path, path: &Path) -> Option<PathBuf> {
    let canonical_path = path.canonicalize().ok()?;
    canonical_path.starts_with(canonical_root).then_some(canonical_path)
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
    fn is_request_path_safe_rejects_traversal() {
        assert!(!is_request_path_safe("../../../etc/passwd"));
        assert!(!is_request_path_safe("/static/../etc/passwd"));
    }

    #[test]
    fn is_request_path_safe_accepts_valid_paths() {
        assert!(is_request_path_safe("/index.html"));
        assert!(is_request_path_safe("/static/app.js"));
        assert!(is_request_path_safe("/static/app..js"));
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

    #[test]
    fn render_directory_listing_html_sorts_entries_and_adds_parent_link() {
        let html = render_directory_listing_html(
            "/nested",
            &[
                DirectoryListingEntry { name: "z-last.txt".to_string(), is_dir: false },
                DirectoryListingEntry { name: "assets".to_string(), is_dir: true },
                DirectoryListingEntry { name: "a-first.txt".to_string(), is_dir: false },
            ],
        );

        assert!(html.contains("<h1>Index of /nested/</h1>"));
        assert!(html.contains("<li><a href=\"/\">../</a></li>"));
        assert!(html.contains("<li><a href=\"/nested/assets/\">assets/</a></li>"));
        assert!(
            html.find("href=\"/nested/a-first.txt\"").expect("listing should contain a-first.txt")
                < html.find("href=\"/nested/assets/\"").expect("listing should contain assets/")
        );
        assert!(
            html.find("href=\"/nested/assets/\"").expect("listing should contain assets/")
                < html
                    .find("href=\"/nested/z-last.txt\"")
                    .expect("listing should contain z-last.txt")
        );
    }

    #[test]
    fn render_directory_listing_html_escapes_display_names_and_hrefs() {
        let html = render_directory_listing_html(
            "/",
            &[DirectoryListingEntry { name: "a & b<#>.txt".to_string(), is_dir: false }],
        );

        assert!(html.contains("<h1>Index of /</h1>"));
        assert!(html.contains("href=\"/a%20&amp;%20b%3C%23%3E.txt\""));
        assert!(html.contains(">a &amp; b&lt;#&gt;.txt</a>"));
    }
}
