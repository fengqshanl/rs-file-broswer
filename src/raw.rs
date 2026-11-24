//! Same-origin delivery for HTML previews and the files they reference.
//!
//! A previewed document used to be handed to the browser as a `blob:` URL with
//! an injected `<meta>` CSP. Two things went wrong with that, and both were
//! invisible from the outside:
//!
//! * a `blob:` document inherits the *creating* document's CSP (policy
//!   container inheritance for local schemes) and every policy in the list is
//!   enforced, so the app's own `style-src 'self'` vetoed the `'unsafe-inline'`
//!   the preview asked for — HTML files rendered unstyled;
//! * relative URLs resolve against the blob URL, so a sibling `style.css`, an
//!   `img/logo.png` or an `@font-face` source never resolved at all.
//!
//! Serving the document from its real path fixes both at once: relative URLs
//! keep their meaning, and the response carries its own CSP instead of
//! inheriting one. Access is gated by the same short-lived directory ticket the
//! media streams use. The ticket rides in the path because relative subresource
//! requests cannot carry a query string, and a directory ticket already covers
//! every descendant the page may reference.
//!
//! The document is still framed with `sandbox` (without `allow-same-origin`), so
//! it runs in an opaque origin: scripts cannot reach the app's DOM, storage or
//! bearer token, and the CSP keeps every request on the app's own origin.

use std::{io, sync::Arc};

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue},
    response::Response,
};
use futures_util::stream::try_unfold;
use tokio::io::AsyncReadExt;

use crate::{
    error::ApiError, files::resolve_file, preview::content_type_for, state::AppState, stream,
};

/// Enough to see a BOM or the document's own `<meta charset>` declaration.
const HEAD_BYTES: usize = 8 * 1024;
/// How far into the head to look for a charset label.
const SNIFF_BYTES: usize = 4 * 1024;
const RAW_CHUNK: usize = 64 * 1024;

/// `GET /raw/{ticket}/{*path}`: a preview document or one of its assets.
///
/// Documents sign their directory, so a page and everything next to it share
/// one ticket. `verify_ticket` walks the parent chain for directory tickets and
/// rejects anything outside that subtree.
pub async fn serve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((ticket, path)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    stream::verify_ticket(&state, &ticket, &path)?;
    let (fs_path, _clean, name, resolved_size) = resolve_file(&state, &path).await?;
    let mut file = tokio::fs::File::open(&fs_path)
        .await
        .map_err(|_| ApiError::not_found("文件不存在或无法读取"))?;
    let size = file
        .metadata()
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(resolved_size);
    let content_type = content_type_for(&name);
    let is_document = content_type == "text/html";

    // The head doubles as an encoding hint and as the first body bytes, so a
    // document is never buffered twice or read into memory as a whole.
    let head = read_head(&mut file, size.min(HEAD_BYTES as u64) as usize).await?;
    let charset = if is_document {
        sniff_charset(&head).unwrap_or_else(|| "utf-8".to_string())
    } else {
        String::new()
    };
    let consumed = head.len() as u64;

    let permit = state
        .preview_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("预览并发控制器已关闭"))?;
    let body_state = RawBody {
        file,
        pending: head,
        remaining: size.saturating_sub(consumed),
        limiter: state.download_limiter.clone(),
        _permit: permit,
    };
    let body = try_unfold(body_state, |mut state| async move {
        if !state.pending.is_empty() {
            let chunk = std::mem::take(&mut state.pending);
            return Ok::<_, io::Error>(Some((Bytes::from(chunk), state)));
        }
        if state.remaining == 0 {
            return Ok(None);
        }
        let want = RAW_CHUNK.min(state.remaining as usize);
        let mut buffer = vec![0u8; want];
        let read = state.file.read(&mut buffer).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.truncate(read);
        state.remaining -= read as u64;
        state.limiter.wait(read).await;
        Ok(Some((Bytes::from(buffer), state)))
    });

    let mut response = Response::new(Body::from_stream(body));
    let out = response.headers_mut();
    let value = if is_document {
        format!("{content_type}; charset={charset}")
    } else {
        content_type.to_string()
    };
    out.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&value).map_err(|_| ApiError::internal("构造预览内容类型失败"))?,
    );
    out.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string())
            .map_err(|_| ApiError::internal("构造文件大小响应头失败"))?,
    );
    out.insert(
        header::CACHE_CONTROL,
        // The document itself may be edited between two previews; its assets
        // are worth a short cache so a long page does not refetch them.
        if is_document {
            HeaderValue::from_static("private, no-cache")
        } else {
            HeaderValue::from_static("private, max-age=60")
        },
    );
    out.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // The sandboxed frame has an opaque origin, so even a same-host `<img>` or
    // `@font-face` is a cross-origin request; without this a page's own fonts
    // (CORS mode) would be rejected. The ticket, not this header, is the gate.
    out.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if is_document {
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok());
        let policy = HeaderValue::from_str(&html_csp(host))
            .map_err(|_| ApiError::internal("构造预览策略失败"))?;
        out.insert(header::CONTENT_SECURITY_POLICY, policy);
    }
    Ok(response)
}

struct RawBody {
    file: tokio::fs::File,
    pending: Vec<u8>,
    remaining: u64,
    limiter: Arc<crate::rate::RateLimiter>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

async fn read_head(file: &mut tokio::fs::File, want: usize) -> Result<Vec<u8>, ApiError> {
    let mut head = vec![0u8; want];
    let mut filled = 0;
    while filled < want {
        match file.read(&mut head[filled..]).await {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => return Err(ApiError::internal("读取预览文件失败")),
        }
    }
    head.truncate(filled);
    Ok(head)
}

/// The CSP a previewed document runs under.
///
/// `default-src 'none'` keeps the page offline; the only origins allowed are the
/// app's own (so relative stylesheets, images, fonts, scripts and a local
/// `fetch('data.json')` all load) plus `data:`/`blob:` for inlined assets.
/// `'self'` cannot be used for those because the sandboxed frame has an opaque
/// origin, which matches no origin at all, so the host is spelled out from the
/// request's `Host` header instead.
///
/// `base-uri 'none'` neutralises a `<base href="/">` left in an exported page:
/// with the tag ignored, relative URLs resolve against the document's real
/// path — which is exactly the directory the ticket covers. `frame-ancestors`
/// stays `'self'` (evaluated against the response origin, not the sandboxed
/// frame) so no third-party page can frame the document.
pub fn html_csp(host: Option<&str>) -> String {
    let sources = match host.and_then(safe_host) {
        Some(host) => format!("http://{host} https://{host}"),
        None => "'self'".to_string(),
    };
    format!(
        "default-src 'none'; style-src 'unsafe-inline' {sources}; \
         script-src 'unsafe-inline' 'unsafe-eval' {sources}; \
         img-src {sources} data: blob:; font-src {sources} data:; \
         media-src {sources} data: blob:; frame-src {sources}; \
         connect-src {sources}; object-src 'none'; form-action 'none'; \
         base-uri 'none'; frame-ancestors 'self';"
    )
}

/// A `Host` header value is only reused verbatim once it is known to contain
/// nothing a header cannot hold (or a CSP source expression cannot parse).
fn safe_host(host: &str) -> Option<String> {
    if host.is_empty() || host.len() > 128 {
        return None;
    }
    let valid = host.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'-' | b'[' | b']')
    });
    valid.then(|| host.to_string())
}

/// Reads the label a document declares for itself. Returning it instead of
/// re-encoding keeps the original bytes untouched, which matters for the many
/// GBK/GB18030 exports that would otherwise render as mojibake.
fn sniff_charset(head: &[u8]) -> Option<String> {
    if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Some("utf-8".to_string());
    }
    if head.starts_with(&[0xFF, 0xFE]) {
        return Some("utf-16le".to_string());
    }
    if head.starts_with(&[0xFE, 0xFF]) {
        return Some("utf-16be".to_string());
    }
    let window = &head[..head.len().min(SNIFF_BYTES)];
    let lower = String::from_utf8_lossy(window).to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find("charset") {
        let mut index = cursor + found + "charset".len();
        let bytes = lower.as_bytes();
        while index < bytes.len() && matches!(bytes[index], b' ' | b'=' | b'"' | b'\'') {
            index += 1;
        }
        let mut end = index;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric()
                || matches!(bytes[end], b'-' | b'_' | b'.'))
        {
            end += 1;
        }
        if end > index {
            return Some(lower[index..end].to_string());
        }
        cursor = cursor + found + "charset".len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_declarations_are_read_back() {
        let utf8 = b"<!doctype html><meta charset=\"UTF-8\">";
        assert_eq!(sniff_charset(utf8).as_deref(), Some("utf-8"));

        let gbk = b"<html><head><meta http-equiv=Content-Type content=\"text/html; charset=gb2312\">";
        assert_eq!(sniff_charset(gbk).as_deref(), Some("gb2312"));

        let single = b"<meta charset='big5'>";
        assert_eq!(sniff_charset(single).as_deref(), Some("big5"));

        assert_eq!(sniff_charset(b"<html><body>no meta</body></html>"), None);
        assert_eq!(
            sniff_charset(&[0xEF, 0xBB, 0xBF, b'<']).as_deref(),
            Some("utf-8")
        );
    }

    #[test]
    fn csp_names_the_request_host_and_stays_offline() {
        let policy = html_csp(Some("127.0.0.1:8090"));
        assert!(policy.contains("style-src 'unsafe-inline' http://127.0.0.1:8090"));
        assert!(policy.starts_with("default-src 'none'"));
        assert!(policy.contains("frame-ancestors 'self'"));
        // Only this host is reachable: no wildcard, no bare scheme.
        assert!(!policy.contains('*'));
        assert!(!policy.contains("http: "));

        // A header-injection attempt falls back to the useless-but-safe 'self'.
        let hostile = html_csp(Some("evil.example; script-src *"));
        assert!(hostile.contains("style-src 'unsafe-inline' 'self'"));
        assert!(!hostile.contains("evil.example"));
    }
}
