//! Range-capable, unencrypted byte streaming for media previews.
//!
//! Every other endpoint wraps its payload in an AES-GCM envelope, which is the
//! right trade-off for small control messages: the browser can decrypt them in
//! one go. It is the wrong trade-off for photos and videos, because
//!
//! * a `<video>` element cannot seek inside a stream it cannot address, so the
//!   old flow had to download the whole file before playback started;
//! * the sandboxed `fetch` + `crypto.subtle.decrypt` loop costs a round trip
//!   through WebCrypto per 64 KiB frame, and JS runs that on the main thread.
//!
//! Here the transport already provides confidentiality (TLS for a standalone
//! server, loopback-only HTTP for the desktop companion), so bytes travel as
//! plain `200`/`206` responses and the element streams them natively. Access is
//! gated by a short-lived HMAC ticket instead of the bearer token, because a
//! native `<img src>` cannot attach headers:
//!
//! * `POST /api/stream/ticket` is an ordinary encrypted, authenticated call;
//! * the ticket signs *scope + path + expiry* with a per-process secret, so it
//!   cannot be widened to another path and dies after five minutes;
//! * a directory-scoped ticket lets one listing page reuse a single ticket for
//!   every thumbnail and preview below that folder.

use std::{
    io,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, Query, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::Response,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::stream::try_unfold;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::{
    api::{encrypted_response, read_encrypted},
    auth::AuthSession,
    crypto,
    error::ApiError,
    files::{clean_rel, resolve_dir, resolve_file},
    preview::content_type_for,
    state::AppState,
};

/// 256 KiB per read keeps the per-chunk bookkeeping negligible while a paused
/// transfer still stops quickly.
const STREAM_CHUNK: usize = 256 * 1024;
/// A ticket only has to survive the moment between the API call and the browser
/// opening the media element. Five minutes covers a slow connection.
const TICKET_TTL_SECS: u64 = 300;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    File,
    Dir,
}

impl Scope {
    fn code(self) -> char {
        match self {
            Self::File => 'f',
            Self::Dir => 'd',
        }
    }

    fn parse(code: &str) -> Option<Self> {
        match code {
            "f" => Some(Self::File),
            "d" => Some(Self::Dir),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct TicketRequest {
    path: String,
    #[serde(default)]
    dir: bool,
}

/// Issues the ticket a media element (or thumbnail `<img>`) will present.
pub async fn issue_ticket(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: TicketRequest = read_encrypted(&session, &aad, body).await?;

    // Sign the canonical path: the browser may repeat it with a leading slash,
    // and the signature must not depend on that spelling.
    let (scope, clean) = if request.dir {
        let (_path, clean) = resolve_dir(&state, &request.path).await?;
        (Scope::Dir, clean)
    } else {
        let (_path, clean, _name, _size) = resolve_file(&state, &request.path).await?;
        (Scope::File, clean)
    };

    let expires_at = now_secs().saturating_add(TICKET_TTL_SECS);
    let signature = sign(&state.stream_secret, scope, &clean, expires_at);
    let ticket = format!("v1.{expires_at}.{}.{signature}", scope.code());
    encrypted_response(
        &session,
        &aad,
        json!({ "ticket": ticket, "expires_at": expires_at, "path": clean }),
    )
}

#[derive(Deserialize)]
pub struct StreamQuery {
    pub path: String,
    pub ticket: String,
}

pub async fn stream(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    verify_ticket(&state, &query.ticket, &query.path)?;
    let (path, _clean, name, resolved_size) = resolve_file(&state, &query.path).await?;
    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found("文件不存在或无法读取"))?;
    let size = file
        .metadata()
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(resolved_size);

    let range = match parse_range(&headers, size) {
        Ok(range) => range,
        Err(()) => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
            response.headers_mut().insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{size}"))
                    .map_err(|_| ApiError::internal("构造范围响应头失败"))?,
            );
            return Ok(response);
        }
    };

    let (start, end) = match range {
        Some(range) => (range.start, range.end),
        None if size == 0 => (0, 0),
        None => (0, size - 1),
    };
    let length = if size == 0 { 0 } else { end - start + 1 };

    if start > 0 {
        file.seek(io::SeekFrom::Start(start))
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
    }

    let permit = state
        .preview_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("预览并发控制器已关闭"))?;
    let stream = try_unfold(
        (file, length, state.download_limiter.clone(), permit),
        |(mut file, remaining, limiter, permit)| async move {
            if remaining == 0 {
                return Ok::<_, io::Error>(None);
            }
            let want = STREAM_CHUNK.min(remaining as usize);
            let mut buffer = vec![0u8; want];
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                return Ok(None);
            }
            buffer.truncate(read);
            limiter.wait(read).await;
            Ok(Some((
                Bytes::from(buffer),
                (file, remaining - read as u64, limiter, permit),
            )))
        },
    );

    let mut response = Response::new(Body::from_stream(stream));
    let status = if range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    *response.status_mut() = status;
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type_for(&name))
            .map_err(|_| ApiError::internal("构造预览类型失败"))?,
    );
    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string())
            .map_err(|_| ApiError::internal("构造文件大小响应头失败"))?,
    );
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(range) = range {
        response_headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {}-{}/{}", range.start, range.end, size))
                .map_err(|_| ApiError::internal("构造范围响应头失败"))?,
        );
    }
    // Media is immutable enough for a short browser cache: re-seeking a video
    // then costs nothing, and a stale entry expires well before a re-upload.
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=60"),
    );
    response_headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

#[derive(Clone, Copy)]
struct ByteRange {
    start: u64,
    end: u64,
}

/// Parses a single-range `Range` header. Multi-range requests fall back to the
/// first range, which every browser player accepts.
fn parse_range(headers: &HeaderMap, size: u64) -> Result<Option<ByteRange>, ()> {
    let Some(value) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(None);
    };
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Err(());
    };
    let spec = spec.split(',').next().unwrap_or("").trim();
    let (start_raw, end_raw) = spec.split_once('-').ok_or(())?;
    if size == 0 {
        return Err(());
    }

    let (start, end) = if start_raw.is_empty() {
        // `bytes=-N`: the last N bytes.
        let suffix: u64 = end_raw.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = start_raw.parse().map_err(|_| ())?;
        let end = if end_raw.is_empty() {
            size - 1
        } else {
            end_raw.parse::<u64>().map_err(|_| ())?.min(size - 1)
        };
        (start, end)
    };
    if start > end || start >= size {
        return Err(());
    }
    Ok(Some(ByteRange { start, end }))
}

/// Checks a ticket against the requested path. Directory tickets cover every
/// descendant, which is what lets one listing share a single ticket.
pub fn verify_ticket(state: &AppState, ticket: &str, request_path: &str) -> Result<(), ApiError> {
    let clean = clean_rel(request_path).map_err(|_| ApiError::forbidden("预览凭证无效"))?;
    let mut parts = ticket.split('.');
    if parts.next() != Some("v1") {
        return Err(ApiError::forbidden("预览凭证无效"));
    }
    let expires_at: u64 = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| ApiError::forbidden("预览凭证无效"))?;
    let scope = parts
        .next()
        .and_then(Scope::parse)
        .ok_or_else(|| ApiError::forbidden("预览凭证无效"))?;
    let signature = parts.next().ok_or_else(|| ApiError::forbidden("预览凭证无效"))?;
    if parts.next().is_some() {
        return Err(ApiError::forbidden("预览凭证无效"));
    }
    if now_secs() > expires_at {
        return Err(ApiError::forbidden("预览凭证已过期，请重试"));
    }

    let matches = |candidate: &str| {
        constant_time_eq(
            sign(&state.stream_secret, scope, candidate, expires_at).as_bytes(),
            signature.as_bytes(),
        )
    };

    match scope {
        Scope::File => {
            if matches(&clean) {
                Ok(())
            } else {
                Err(ApiError::forbidden("预览凭证无效"))
            }
        }
        Scope::Dir => {
            let mut candidate = clean.as_str();
            loop {
                if matches(candidate) {
                    return Ok(());
                }
                match candidate.rfind('/') {
                    Some(index) => candidate = &candidate[..index],
                    None => break,
                }
            }
            // The root folder signs the empty path, so it is checked last.
            if matches("") {
                return Ok(());
            }
            Err(ApiError::forbidden("预览凭证无效"))
        }
    }
}

fn sign(secret: &[u8; 32], scope: Scope, path: &str, expires_at: u64) -> String {
    let message = format!("{}|{}|{}", scope.code(), path, expires_at);
    let digest = hmac_sha256(secret, message.as_bytes());
    // 128 bits of tag is far beyond what a five-minute ticket needs.
    URL_SAFE_NO_PAD.encode(&digest[..16])
}

/// HMAC-SHA256 (RFC 2104) built on `sha2`, so the ticket secret needs no extra
/// dependency in the image.
fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_bound_to_path_and_expiry() {
        let secret = [7u8; 32];
        let signature = sign(&secret, Scope::File, "photos/a.jpg", 1_000);
        assert_eq!(signature, sign(&secret, Scope::File, "photos/a.jpg", 1_000));
        assert_ne!(signature, sign(&secret, Scope::File, "photos/b.jpg", 1_000));
        assert_ne!(signature, sign(&secret, Scope::File, "photos/a.jpg", 1_001));
        assert_ne!(signature, sign(&secret, Scope::Dir, "photos/a.jpg", 1_000));
    }

    #[test]
    fn hmac_matches_the_reference_vector() {
        // Same key/data shape as RFC 4231 case 1, with the 32-byte key this
        // module always uses.
        let key = [0x0bu8; 32];
        let digest = hmac_sha256(&key, b"Hi There");
        let expected = "198a607eb44bfbc69903a0f1cf2bbdc5ba0aa3f3d9ae3c1c7a3b1696a0b68cf7";
        let actual = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn ranges_cover_the_usual_player_requests() {
        let mut headers = HeaderMap::new();
        assert!(parse_range(&headers, 100).unwrap().is_none());

        headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-"));
        let range = parse_range(&headers, 100).unwrap().unwrap();
        assert_eq!((range.start, range.end), (0, 99));

        headers.insert(header::RANGE, HeaderValue::from_static("bytes=10-19"));
        let range = parse_range(&headers, 100).unwrap().unwrap();
        assert_eq!((range.start, range.end), (10, 19));

        headers.insert(header::RANGE, HeaderValue::from_static("bytes=-20"));
        let range = parse_range(&headers, 100).unwrap().unwrap();
        assert_eq!((range.start, range.end), (80, 99));

        headers.insert(header::RANGE, HeaderValue::from_static("bytes=50-9999"));
        let range = parse_range(&headers, 100).unwrap().unwrap();
        assert_eq!((range.start, range.end), (50, 99));

        headers.insert(header::RANGE, HeaderValue::from_static("bytes=100-"));
        assert!(parse_range(&headers, 100).is_err());
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-"));
        assert!(parse_range(&headers, 0).is_err());
        headers.insert(header::RANGE, HeaderValue::from_static("items=0-1"));
        assert!(parse_range(&headers, 100).is_err());
    }
}
