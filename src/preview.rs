use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, Path as UrlPath, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::stream;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::{
    api::{encrypted_response, read_encrypted},
    auth::AuthSession,
    crypto,
    error::{log, ApiError},
    files::{resolve_file, rfc5987},
    state::AppState,
};

pub const PREVIEW_TOKEN_TTL: Duration = Duration::from_secs(3600);
pub const PREVIEW_CHUNK: usize = 64 * 1024;
/// Caps a single ranged response so a hostile client cannot pin a huge
/// read per request; browsers re-request the next range as needed.
const MAX_PREVIEW_RANGE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct PreviewToken {
    pub path: PathBuf,
    pub name: String,
    pub session: String,
    pub content_type: &'static str,
    pub expires_at: Instant,
}

#[derive(Deserialize)]
struct PreviewTokenRequest {
    path: String,
}

pub async fn create_token(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: PreviewTokenRequest = read_encrypted(&session, &aad, body).await?;
    let (path, clean, name, size) = resolve_file(&state, &request.path).await?;

    let token = crypto::random_b64(32);
    let content_type = content_type_for(&name);
    state.preview_tokens.insert(
        token.clone(),
        PreviewToken {
            path,
            name: name.clone(),
            session: session.token.clone(),
            content_type,
            expires_at: Instant::now() + PREVIEW_TOKEN_TTL,
        },
    );
    log(&format!("签发预览令牌 file={clean}"));

    encrypted_response(
        &session,
        &aad,
        json!({
            "url": format!("/preview/{token}"),
            "name": name,
            "path": clean,
            "size": size,
            "content_type": content_type,
            "expires_in": PREVIEW_TOKEN_TTL.as_secs(),
        }),
    )
}

pub async fn serve(
    State(state): State<Arc<AppState>>,
    UrlPath(token): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let entry = state
        .preview_tokens
        .get(&token)
        .map(|entry| entry.value().clone())
        .ok_or_else(|| ApiError::not_found("预览链接已过期，请重新打开文件"))?;

    if Instant::now() >= entry.expires_at || !state.sessions.contains_key(&entry.session) {
        state.preview_tokens.remove(&token);
        return Err(ApiError::not_found("预览链接已过期，请重新登录后打开文件"));
    }

    let mut file = tokio::fs::File::open(&entry.path)
        .await
        .map_err(|_| ApiError::not_found("文件不存在或无法读取"))?;
    let size = file
        .metadata()
        .await
        .map_err(|_| ApiError::not_found("文件不存在或无法读取"))?
        .len();

    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let range = range_header.and_then(|value| parse_range(value, size));
    if range_header.is_some() && range.is_none() && size > 0 {
        let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes */{size}"))
                .map_err(|_| ApiError::internal("构造 Range 响应失败"))?,
        );
        return Ok(response);
    }

    let (status, start, end) = match range {
        Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end),
        None if size == 0 => (StatusCode::OK, 0, 0),
        None => (StatusCode::OK, 0, size - 1),
    };
    let length = if size == 0 { 0 } else { end - start + 1 };

    let permit = state
        .preview_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("预览并发控制器已关闭"))?;

    let body = if length == 0 {
        drop(permit);
        Body::empty()
    } else {
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
        let limiter = state.download_limiter.clone();
        let stream_state = PreviewStream {
            file,
            remaining: length,
            limiter,
            _permit: permit,
        };
        let body_stream = stream::try_unfold(stream_state, |mut state| async move {
            if state.remaining == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            let want = PREVIEW_CHUNK.min(state.remaining as usize);
            let mut buffer = vec![0u8; want];
            let read = state.file.read(&mut buffer).await?;
            if read == 0 {
                return Err(std::io::Error::other("预览读取提前结束"));
            }
            buffer.truncate(read);
            state.remaining -= read as u64;
            state.limiter.wait(read).await;
            Ok(Some((Bytes::from(buffer), state)))
        });
        Body::from_stream(body_stream)
    };

    let mut response = Response::new(body);
    *response.status_mut() = status;
    let response_headers = response.headers_mut();
    let content_type = if is_textual(entry.content_type) {
        format!("{}; charset=utf-8", entry.content_type)
    } else {
        entry.content_type.to_string()
    };
    insert(
        response_headers,
        header::CONTENT_TYPE,
        &content_type,
        "Content-Type",
    )?;
    insert(
        response_headers,
        header::CONTENT_LENGTH,
        &length.to_string(),
        "Content-Length",
    )?;
    insert(
        response_headers,
        header::ACCEPT_RANGES,
        "bytes",
        "Accept-Ranges",
    )?;
    if status == StatusCode::PARTIAL_CONTENT {
        insert(
            response_headers,
            header::CONTENT_RANGE,
            &format!("bytes {start}-{end}/{size}"),
            "Content-Range",
        )?;
    }
    insert(
        response_headers,
        header::CACHE_CONTROL,
        "private, no-store",
        "Cache-Control",
    )?;
    insert(
        response_headers,
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff",
        "X-Content-Type-Options",
    )?;
    insert(
        response_headers,
        header::REFERRER_POLICY,
        "no-referrer",
        "Referrer-Policy",
    )?;
    insert(
        response_headers,
        HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
        "Cross-Origin-Resource-Policy",
    )?;
    insert(
        response_headers,
        header::X_FRAME_OPTIONS,
        "SAMEORIGIN",
        "X-Frame-Options",
    )?;
    insert(
        response_headers,
        header::CONTENT_DISPOSITION,
        &format!("inline; filename*=UTF-8''{}", rfc5987(&entry.name)),
        "Content-Disposition",
    )?;

    // Untrusted HTML/SVG/XML runs on an opaque origin: scripts may render but
    // can never touch this app's DOM, storage, or authenticated API surface.
    if matches!(
        entry.content_type,
        "text/html" | "image/svg+xml" | "application/xml"
    ) {
        insert(
            response_headers,
            HeaderName::from_static("content-security-policy"),
            "sandbox allow-scripts allow-forms allow-popups allow-modals; default-src 'unsafe-inline' data: blob:",
            "Content-Security-Policy",
        )?;
    }

    Ok(response)
}

fn is_textual(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json" | "application/xml" | "image/svg+xml"
        )
}

fn insert(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: &str,
    label: &str,
) -> Result<(), ApiError> {
    let header_value = HeaderValue::from_str(value)
        .map_err(|_| ApiError::internal(format!("构造 {label} 响应头失败")))?;
    headers.insert(name, header_value);
    Ok(())
}

struct PreviewStream {
    file: tokio::fs::File,
    remaining: u64,
    limiter: Arc<crate::rate::RateLimiter>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Parses a single `bytes=` range. Multi-range requests are ignored and the
/// full file is served, which browsers handle fine.
pub fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?.trim();
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (start_raw, end_raw) = spec.split_once('-')?;
    let start_raw = start_raw.trim();
    let end_raw = end_raw.trim();

    if start_raw.is_empty() {
        let suffix: u64 = end_raw.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = size.saturating_sub(suffix);
        return Some((start, size - 1));
    }

    let start: u64 = start_raw.parse().ok()?;
    if start >= size {
        return None;
    }
    let end = if end_raw.is_empty() {
        size - 1
    } else {
        let end: u64 = end_raw.parse().ok()?;
        if end < start {
            return None;
        }
        end.min(size - 1)
    };
    let end = end.min(start.saturating_add(MAX_PREVIEW_RANGE_BYTES - 1));
    Some((start, end))
}

pub fn content_type_for(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "txt" | "log" | "ini" | "conf" | "cfg" | "env" | "yaml" | "yml" | "toml" | "rs" | "py"
        | "sh" | "bash" | "zsh" | "c" | "h" | "cpp" | "hpp" | "go" | "java" | "kt" | "swift"
        | "sql" | "vue" | "svelte" | "lock" => "text/plain",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "doc" => "application/msword",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

pub fn cleanup_expired(state: &AppState) {
    let now = Instant::now();
    state
        .preview_tokens
        .retain(|_, entry| now < entry.expires_at && state.sessions.contains_key(&entry.session));
}

pub fn revoke_session(state: &AppState, session: &str) {
    state
        .preview_tokens
        .retain(|_, entry| entry.session != session);
}

pub fn revoke_other_sessions(state: &AppState, keep_session: &str) {
    state
        .preview_tokens
        .retain(|_, entry| entry.session == keep_session);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=0-0", 1000), Some((0, 0)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=5-1", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-9", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }

    #[test]
    fn range_is_capped() {
        let (start, end) = parse_range("bytes=0-", 100 * 1024 * 1024).unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, MAX_PREVIEW_RANGE_BYTES - 1);
    }

    #[test]
    fn mime_mapping() {
        assert_eq!(content_type_for("a.MP4"), "video/mp4");
        assert_eq!(content_type_for("a.pdf"), "application/pdf");
        assert_eq!(content_type_for("a.html"), "text/html");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
    }
}
