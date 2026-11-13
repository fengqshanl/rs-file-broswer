use std::{io, sync::Arc};

use aes_gcm::{Aes256Gcm, KeyInit};
use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, State},
    http::{header, HeaderName, HeaderValue, Method},
    response::Response,
};
use futures_util::stream::try_unfold;
use serde::Deserialize;
use tokio::io::AsyncReadExt;

use crate::{
    api::read_encrypted,
    auth::AuthSession,
    crypto,
    error::ApiError,
    files::{resolve_file, DOWNLOAD_CHUNK},
    state::AppState,
};

const PREVIEW_CHUNK: usize = DOWNLOAD_CHUNK;

#[derive(Deserialize)]
struct PreviewStreamRequest {
    path: String,
}

/// Streams preview bytes as AES-256-GCM frames. Native `<img>/<video>/<iframe>`
/// elements cannot decrypt application-layer frames, so the frontend fetches
/// this endpoint with `fetch()`, authenticates with the normal bearer token and
/// decrypts in memory before creating a short-lived blob URL.
pub async fn stream(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: PreviewStreamRequest = read_encrypted(&session, &aad, body).await?;
    let (path, _clean, name, resolved_size) = resolve_file(&state, &request.path).await?;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found("文件不存在或无法读取"))?;
    let size = file
        .metadata()
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(resolved_size);
    let content_type = content_type_for(&name);

    let permit = state
        .preview_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("预览并发控制器已关闭"))?;
    let cipher =
        Aes256Gcm::new_from_slice(&session.key).map_err(|_| ApiError::internal("会话密钥无效"))?;
    // 18 random bytes encode to 24 base64 characters without padding, safe in
    // an HTTP header. The id binds every frame to this response.
    let stream_id = crypto::random_b64(18);

    let stream_state = PreviewStream {
        file,
        cipher,
        seq: 0,
        stream_id: stream_id.clone(),
        limiter: state.download_limiter.clone(),
        _permit: permit,
    };
    let stream = try_unfold(stream_state, |mut stream| async move {
        let mut buffer = vec![0u8; PREVIEW_CHUNK];
        let read = stream.file.read(&mut buffer).await?;
        if read == 0 {
            return Ok::<_, io::Error>(None);
        }
        buffer.truncate(read);

        stream.limiter.wait(4 + 12 + buffer.len() + 16).await;
        let aad = format!("pv:{}:{}", stream.stream_id, stream.seq);
        let frame = crypto::seal_frame(&stream.cipher, &aad, &buffer)
            .map_err(|_| io::Error::other("预览分块加密失败"))?;
        stream.seq += 1;
        Ok(Some((Bytes::from(frame), stream)))
    });

    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("x-encrypted"),
        HeaderValue::from_static("aes-256-gcm"),
    );
    headers.insert(
        HeaderName::from_static("x-stream-id"),
        HeaderValue::from_str(&stream_id).map_err(|_| ApiError::internal("构造预览流标识失败"))?,
    );
    headers.insert(
        HeaderName::from_static("x-file-size"),
        HeaderValue::from_str(&size.to_string())
            .map_err(|_| ApiError::internal("构造文件大小响应头失败"))?,
    );
    headers.insert(
        HeaderName::from_static("x-chunk-size"),
        HeaderValue::from_str(&PREVIEW_CHUNK.to_string())
            .map_err(|_| ApiError::internal("构造分块大小响应头失败"))?,
    );
    headers.insert(
        HeaderName::from_static("x-preview-content-type"),
        HeaderValue::from_static(content_type),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    Ok(response)
}

struct PreviewStream {
    file: tokio::fs::File,
    cipher: Aes256Gcm,
    seq: u64,
    stream_id: String,
    limiter: Arc<crate::rate::RateLimiter>,
    _permit: tokio::sync::OwnedSemaphorePermit,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_mapping() {
        assert_eq!(content_type_for("a.MP4"), "video/mp4");
        assert_eq!(content_type_for("a.pdf"), "application/pdf");
        assert_eq!(content_type_for("a.html"), "text/html");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
    }
}
