use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use aes_gcm::{
    aead::{Aead, AeadCore, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, Path as UrlPath, State},
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    response::Response,
};
use futures_util::stream::try_unfold;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, Mutex},
};

use crate::{
    api::{encrypted_response, read_encrypted},
    archive::{self, ArchiveMessage, ARCHIVE_END_MARKER},
    auth::AuthSession,
    crypto,
    error::{log, ApiError},
    state::{AppState, UploadInner, UploadSession},
};

pub const DOWNLOAD_CHUNK: usize = 64 * 1024;
pub const UPLOAD_CHUNK_MAX: usize = 4 * 1024 * 1024;
const CHUNK_BODY_LIMIT: usize = 4 + 12 + UPLOAD_CHUNK_MAX + 16 + 64;
const MAX_LIST_ENTRIES: usize = 50_000;
const MAX_ENSURE_DIRS: usize = 20_000;
/// A scan stays valid for this long, so paging through a large folder costs
/// one directory walk instead of one per page.
const LISTING_TTL: Duration = Duration::from_secs(15);
/// Caps how much memory cached folder scans may occupy (entries are ~100 B).
const MAX_CACHED_LISTINGS: usize = 2;
const LIST_PAGE_LIMIT: usize = 1_000;

#[derive(Deserialize)]
struct ListRequest {
    #[serde(default)]
    path: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_page_size")]
    limit: usize,
}

fn default_page_size() -> usize {
    300
}

#[derive(Deserialize)]
struct MkdirRequest {
    path: String,
}

#[derive(Deserialize)]
struct RenameRequest {
    path: String,
    new_name: String,
}

#[derive(Deserialize)]
struct DeleteRequest {
    path: String,
}

#[derive(Deserialize)]
struct DownloadRequest {
    path: String,
}

#[derive(Deserialize)]
struct DownloadFolderRequest {
    path: String,
}

#[derive(Deserialize)]
struct EnsureDirsRequest {
    paths: Vec<String>,
}

#[derive(Deserialize)]
struct UploadInitRequest {
    dir: String,
    name: String,
    size: u64,
}

#[derive(Deserialize)]
struct UploadIdRequest {
    id: String,
}

#[derive(Clone, Serialize)]
pub struct Entry {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    modified: u64,
}

/// A fully scanned and sorted directory listing, reused across paginated
/// requests so a huge folder is walked once instead of once per page.
pub struct CachedListing {
    pub entries: Arc<Vec<Entry>>,
    pub truncated: bool,
    pub built_at: Instant,
}

struct DownloadStream {
    file: tokio::fs::File,
    cipher: Aes256Gcm,
    seq: u64,
    limiter: Arc<crate::rate::RateLimiter>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

struct FolderStream {
    rx: mpsc::Receiver<ArchiveMessage>,
    cipher: Aes256Gcm,
    seq: u64,
    limiter: Arc<crate::rate::RateLimiter>,
    _permit: tokio::sync::OwnedSemaphorePermit,
    ended: bool,
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn clean_rel(input: &str) -> Result<String, ApiError> {
    let mut parts = Vec::new();
    for component in input.split('/') {
        match component {
            "" | "." => continue,
            ".." => return Err(ApiError::bad("路径不合法")),
            _ => {}
        }
        if component.as_bytes().contains(&0) {
            return Err(ApiError::bad("路径包含非法字符"));
        }
        if component.len() > 255 {
            return Err(ApiError::bad("路径片段过长"));
        }
        parts.push(component);
    }
    Ok(parts.join("/"))
}

fn logical_path(clean: &str) -> String {
    if clean.is_empty() {
        "/".to_string()
    } else {
        format!("/{clean}")
    }
}

fn split_parent(clean: &str) -> Result<(&str, &str), ApiError> {
    let (parent, name) = match clean.rfind('/') {
        Some(index) => (&clean[..index], &clean[index + 1..]),
        None => ("", clean),
    };
    validate_name(name)?;
    Ok((parent, name))
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(ApiError::bad("文件名不合法"));
    }
    if name.contains('/') || name.as_bytes().contains(&0) {
        return Err(ApiError::bad("文件名包含非法字符"));
    }
    if name.len() > 255 {
        return Err(ApiError::bad("文件名过长"));
    }
    Ok(())
}

fn ensure_allowed(state: &AppState, canonical: &Path) -> Result<(), ApiError> {
    let root = state.data_root();
    if !canonical.starts_with(&root) {
        return Err(ApiError::forbidden("路径越界"));
    }
    if canonical == state.cfg.tmp_root || canonical.starts_with(&state.cfg.tmp_root) {
        return Err(ApiError::forbidden("该路径不可访问"));
    }
    Ok(())
}

async fn resolve_dir(state: &AppState, input: &str) -> Result<(PathBuf, String), ApiError> {
    let clean = clean_rel(input)?;
    let root = state.data_root();
    let joined = if clean.is_empty() {
        root
    } else {
        root.join(&clean)
    };
    let canonical = tokio::fs::canonicalize(&joined)
        .await
        .map_err(|_| ApiError::not_found("文件夹不存在"))?;
    ensure_allowed(state, &canonical)?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|_| ApiError::not_found("文件夹不存在"))?;
    if !metadata.is_dir() {
        return Err(ApiError::bad("目标不是文件夹"));
    }
    Ok((canonical, clean))
}

/// Creates every missing component of `clean` below the data root while
/// re-validating existing components against symlink escapes.
async fn ensure_dir_path(state: &AppState, clean: &str) -> Result<bool, ApiError> {
    if clean.is_empty() {
        return Ok(false);
    }

    let mut current = state.data_root();
    let mut created_any = false;

    for component in clean.split('/').filter(|part| !part.is_empty()) {
        let next = current.join(component);
        if next == state.cfg.tmp_root || next.starts_with(&state.cfg.tmp_root) {
            return Err(ApiError::forbidden("该路径不可访问"));
        }

        match tokio::fs::symlink_metadata(&next).await {
            Ok(_) => {
                let canonical = tokio::fs::canonicalize(&next)
                    .await
                    .map_err(|_| ApiError::bad("路径中存在无法访问的项目"))?;
                ensure_allowed(state, &canonical)?;
                let metadata = tokio::fs::metadata(&canonical)
                    .await
                    .map_err(|_| ApiError::bad("路径中存在无法访问的项目"))?;
                if !metadata.is_dir() {
                    return Err(ApiError::conflict("路径中存在同名文件"));
                }
                current = canonical;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                match tokio::fs::create_dir(&next).await {
                    Ok(()) => {
                        let canonical = tokio::fs::canonicalize(&next)
                            .await
                            .map_err(|_| ApiError::bad("路径中存在无法访问的项目"))?;
                        ensure_allowed(state, &canonical)?;
                        current = canonical;
                        created_any = true;
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        let canonical = tokio::fs::canonicalize(&next)
                            .await
                            .map_err(|_| ApiError::bad("路径中存在无法访问的项目"))?;
                        ensure_allowed(state, &canonical)?;
                        let metadata = tokio::fs::metadata(&canonical)
                            .await
                            .map_err(|_| ApiError::bad("路径中存在无法访问的项目"))?;
                        if !metadata.is_dir() {
                            return Err(ApiError::conflict("路径中存在同名文件"));
                        }
                        current = canonical;
                    }
                    Err(err) => return Err(ApiError::internal(err.to_string())),
                }
            }
            Err(err) => return Err(ApiError::internal(err.to_string())),
        }
    }

    Ok(created_any)
}

async fn resolve_dir_create(state: &AppState, clean: &str) -> Result<(PathBuf, String), ApiError> {
    ensure_dir_path(state, clean).await?;
    resolve_dir(state, clean).await
}

pub async fn resolve_file(
    state: &AppState,
    input: &str,
) -> Result<(PathBuf, String, String, u64), ApiError> {
    let clean = clean_rel(input)?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能下载根目录"));
    }
    let joined = state.data_root().join(&clean);
    let canonical = tokio::fs::canonicalize(&joined)
        .await
        .map_err(|_| ApiError::not_found("文件不存在"))?;
    ensure_allowed(state, &canonical)?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|_| ApiError::not_found("文件不存在"))?;
    if !metadata.is_file() {
        return Err(ApiError::bad("该路径不是可下载的文件"));
    }
    let name = clean.rsplit('/').next().unwrap_or("download").to_string();
    Ok((canonical, clean, name, metadata.len()))
}

fn free_space(path: &Path) -> Option<u64> {
    fs2::available_space(path).ok()
}

pub fn rfc5987(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: ListRequest = read_encrypted(&session, &aad, body).await?;
    let (directory, clean) = resolve_dir(&state, &request.path).await?;

    let (entries, truncated) = load_listing(&state, &directory).await?;

    let total = entries.len();
    let limit = request.limit.clamp(1, LIST_PAGE_LIMIT);
    let start = request.offset.min(total);
    let end = start.saturating_add(limit).min(total);
    let page = entries[start..end].to_vec();

    let response = json!({
        "path": logical_path(&clean),
        "entries": page,
        "total": total,
        "offset": start,
        "limit": limit,
        "has_more": end < total,
        "truncated": truncated,
        "free_space": free_space(&directory),
    });
    encrypted_response(&session, &aad, response)
}

async fn load_listing(
    state: &AppState,
    directory: &Path,
) -> Result<(Arc<Vec<Entry>>, bool), ApiError> {
    if let Some(cached) = state.listings.get(directory) {
        if cached.built_at.elapsed() < LISTING_TTL {
            return Ok((cached.entries.clone(), cached.truncated));
        }
    }

    let dir = directory.to_path_buf();
    let tmp_root = state.cfg.tmp_root.clone();
    let (entries, truncated) = tokio::task::spawn_blocking(move || scan_directory(&dir, &tmp_root))
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?
        .map_err(|err| ApiError::internal(err.to_string()))?;
    let entries = Arc::new(entries);

    if state.listings.len() >= MAX_CACHED_LISTINGS {
        state.listings.clear();
    }
    state.listings.insert(
        directory.to_path_buf(),
        CachedListing {
            entries: entries.clone(),
            truncated,
            built_at: Instant::now(),
        },
    );

    Ok((entries, truncated))
}

/// Walks a directory in one blocking pass: a single syscall per regular file
/// instead of the two an async per-entry walk needs. Sorting is done once and
/// reused for every page of the same folder.
fn scan_directory(root: &Path, tmp_root: &Path) -> io::Result<(Vec<Entry>, bool)> {
    let reader = std::fs::read_dir(root)?;
    let mut entries = Vec::new();
    let mut truncated = false;

    for item in reader {
        if entries.len() >= MAX_LIST_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(item) = item else { continue };
        let path = item.path();
        if path == tmp_root {
            continue;
        }
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        let Ok(link_metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let is_symlink = link_metadata.file_type().is_symlink();
        let effective = if is_symlink {
            std::fs::metadata(&path).unwrap_or(link_metadata)
        } else {
            link_metadata
        };
        let is_dir = effective.is_dir();
        let size = if is_dir { 0 } else { effective.len() };
        let modified = effective
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        entries.push(Entry {
            name,
            is_dir,
            is_symlink,
            size,
            modified,
        });
    }

    entries.sort_by_cached_key(|entry| {
        (
            if entry.is_dir { 0u8 } else { 1u8 },
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });

    Ok((entries, truncated))
}

fn invalidate_listings(state: &AppState) {
    state.listings.clear();
}

pub async fn mkdir(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: MkdirRequest = read_encrypted(&session, &aad, body).await?;
    let clean = clean_rel(&request.path)?;
    if clean.is_empty() {
        return Err(ApiError::bad("文件夹名称不能为空"));
    }
    let created = ensure_dir_path(&state, &clean).await?;
    invalidate_listings(&state);
    encrypted_response(
        &session,
        &aad,
        json!({ "ok": true, "path": logical_path(&clean), "created": created }),
    )
}

pub async fn ensure_dirs(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: EnsureDirsRequest = read_encrypted(&session, &aad, body).await?;
    if request.paths.len() > MAX_ENSURE_DIRS {
        return Err(ApiError::bad("一次提交的目录数量过多"));
    }

    let mut seen = HashSet::new();
    let mut created = 0usize;
    for path in request.paths {
        let clean = clean_rel(&path)?;
        if clean.is_empty() || !seen.insert(clean.clone()) {
            continue;
        }
        if ensure_dir_path(&state, &clean).await? {
            created += 1;
        }
    }
    invalidate_listings(&state);

    encrypted_response(&session, &aad, json!({ "ok": true, "created": created }))
}

pub async fn rename(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: RenameRequest = read_encrypted(&session, &aad, body).await?;
    let clean = clean_rel(&request.path)?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能重命名根目录"));
    }
    validate_name(&request.new_name)?;
    let (parent_clean, old_name) = split_parent(&clean)?;
    let (parent, _) = resolve_dir(&state, parent_clean).await?;
    let source = parent.join(old_name);
    if source == state.cfg.tmp_root || source.starts_with(&state.cfg.tmp_root) {
        return Err(ApiError::forbidden("该路径不可访问"));
    }
    tokio::fs::symlink_metadata(&source)
        .await
        .map_err(|_| ApiError::not_found("要重命名的项目不存在"))?;
    let target = parent.join(&request.new_name);
    if tokio::fs::symlink_metadata(&target).await.is_ok() {
        return Err(ApiError::conflict("同名文件或文件夹已存在"));
    }
    tokio::fs::rename(&source, &target)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;
    invalidate_listings(&state);

    let new_clean = if parent_clean.is_empty() {
        request.new_name.clone()
    } else {
        format!("{parent_clean}/{}", request.new_name)
    };
    encrypted_response(
        &session,
        &aad,
        json!({ "ok": true, "path": logical_path(&new_clean) }),
    )
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: DeleteRequest = read_encrypted(&session, &aad, body).await?;
    let clean = clean_rel(&request.path)?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能删除根目录"));
    }
    let (parent_clean, name) = split_parent(&clean)?;
    let (parent, _) = resolve_dir(&state, parent_clean).await?;
    let target = parent.join(name);
    if target == state.cfg.tmp_root || target.starts_with(&state.cfg.tmp_root) {
        return Err(ApiError::forbidden("该路径不可访问"));
    }
    let metadata = tokio::fs::symlink_metadata(&target)
        .await
        .map_err(|_| ApiError::not_found("要删除的项目不存在"))?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        tokio::fs::remove_dir_all(&target)
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
    } else {
        tokio::fs::remove_file(&target)
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
    }
    invalidate_listings(&state);
    encrypted_response(
        &session,
        &aad,
        json!({ "ok": true, "path": logical_path(&clean) }),
    )
}

pub async fn download(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: DownloadRequest = read_encrypted(&session, &aad, body).await?;
    let (path, _clean, name, resolved_size) = resolve_file(&state, &request.path).await?;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;
    let size = file
        .metadata()
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(resolved_size);
    let permit = state
        .download_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("下载并发控制器已关闭"))?;
    let cipher =
        Aes256Gcm::new_from_slice(&session.key).map_err(|_| ApiError::internal("会话密钥无效"))?;

    let stream_state = DownloadStream {
        file,
        cipher,
        seq: 0,
        limiter: state.download_limiter.clone(),
        _permit: permit,
    };
    let stream = try_unfold(stream_state, |mut stream| async move {
        let mut buffer = vec![0u8; DOWNLOAD_CHUNK];
        let read = stream.file.read(&mut buffer).await?;
        if read == 0 {
            return Ok::<_, io::Error>(None);
        }
        buffer.truncate(read);

        stream.limiter.wait(4 + 12 + buffer.len() + 16).await;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let aad = format!("dl:{}", stream.seq);
        let ciphertext = stream
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &buffer,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| io::Error::other("下载分块加密失败"))?;

        let mut frame = Vec::with_capacity(4 + 12 + ciphertext.len());
        frame.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        frame.extend_from_slice(nonce.as_slice());
        frame.extend_from_slice(&ciphertext);
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
        HeaderName::from_static("x-file-size"),
        HeaderValue::from_str(&size.to_string())
            .map_err(|_| ApiError::internal("构造文件大小响应头失败"))?,
    );
    headers.insert(
        HeaderName::from_static("x-chunk-size"),
        HeaderValue::from_str(&DOWNLOAD_CHUNK.to_string())
            .map_err(|_| ApiError::internal("构造分块大小响应头失败"))?,
    );
    headers.insert(
        HeaderName::from_static("x-encrypted"),
        HeaderValue::from_static("aes-256-gcm"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{}", rfc5987(&name)))
            .map_err(|_| ApiError::internal("构造下载文件名失败"))?,
    );
    Ok(response)
}

pub async fn download_folder(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: DownloadFolderRequest = read_encrypted(&session, &aad, body).await?;
    let (directory, clean) = resolve_dir(&state, &request.path).await?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能直接打包下载根目录"));
    }
    let folder_name = clean.rsplit('/').next().unwrap_or("folder").to_string();

    let permit = state
        .download_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("下载并发控制器已关闭"))?;
    let cipher =
        Aes256Gcm::new_from_slice(&session.key).map_err(|_| ApiError::internal("会话密钥无效"))?;

    let rx = archive::spawn_tar_archive(directory, folder_name.clone());
    let stream_state = FolderStream {
        rx,
        cipher,
        seq: 0,
        limiter: state.download_limiter.clone(),
        _permit: permit,
        ended: false,
    };
    let stream = try_unfold(stream_state, |mut stream| async move {
        if stream.ended {
            return Ok::<_, io::Error>(None);
        }

        match stream.rx.recv().await {
            Some(ArchiveMessage::Data(data)) => {
                stream.limiter.wait(4 + 12 + data.len() + 16).await;
                let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
                let aad = format!("dl:{}", stream.seq);
                let ciphertext = stream
                    .cipher
                    .encrypt(
                        &nonce,
                        Payload {
                            msg: data.as_ref(),
                            aad: aad.as_bytes(),
                        },
                    )
                    .map_err(|_| io::Error::other("归档分块加密失败"))?;
                let mut frame = Vec::with_capacity(4 + 12 + ciphertext.len());
                frame.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
                frame.extend_from_slice(nonce.as_slice());
                frame.extend_from_slice(&ciphertext);
                stream.seq += 1;
                Ok(Some((Bytes::from(frame), stream)))
            }
            Some(ArchiveMessage::End) => {
                stream
                    .limiter
                    .wait(4 + 12 + ARCHIVE_END_MARKER.len() + 16)
                    .await;
                let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
                let aad = format!("dl:{}", stream.seq);
                let ciphertext = stream
                    .cipher
                    .encrypt(
                        &nonce,
                        Payload {
                            msg: ARCHIVE_END_MARKER,
                            aad: aad.as_bytes(),
                        },
                    )
                    .map_err(|_| io::Error::other("归档结束标记加密失败"))?;
                let mut frame = Vec::with_capacity(4 + 12 + ciphertext.len());
                frame.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
                frame.extend_from_slice(nonce.as_slice());
                frame.extend_from_slice(&ciphertext);
                stream.seq += 1;
                stream.ended = true;
                Ok(Some((Bytes::from(frame), stream)))
            }
            None => Err(io::Error::other("目录归档流异常中断")),
        }
    });

    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("x-chunk-size"),
        HeaderValue::from_str(&DOWNLOAD_CHUNK.to_string())
            .map_err(|_| ApiError::internal("构造分块大小响应头失败"))?,
    );
    headers.insert(
        HeaderName::from_static("x-encrypted"),
        HeaderValue::from_static("aes-256-gcm"),
    );
    headers.insert(
        HeaderName::from_static("x-encrypted-end"),
        HeaderValue::from_static("1"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename*=UTF-8''{}",
            rfc5987(&format!("{folder_name}.tar"))
        ))
        .map_err(|_| ApiError::internal("构造归档文件名失败"))?,
    );
    Ok(response)
}

pub async fn upload_init(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: UploadInitRequest = read_encrypted(&session, &aad, body).await?;

    if request.size > state.cfg.max_file_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "单个文件不能超过 {} GB",
                state.cfg.max_file_bytes / (1024 * 1024 * 1024)
            ),
        ));
    }
    if state.uploads.len() >= state.cfg.max_active_uploads {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "同时进行的上传任务过多，请稍后重试",
        ));
    }
    validate_name(&request.name)?;
    let dir_clean = clean_rel(&request.dir)?;
    let (directory, _) = resolve_dir_create(&state, &dir_clean).await?;
    invalidate_listings(&state);
    let final_path = directory.join(&request.name);
    if tokio::fs::symlink_metadata(&final_path).await.is_ok() {
        return Err(ApiError::conflict("同名文件或文件夹已存在"));
    }

    let free = free_space(&directory).ok_or_else(|| ApiError::internal("无法读取磁盘剩余空间"))?;
    if free < request.size.saturating_add(state.cfg.disk_reserve_bytes) {
        return Err(ApiError::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "磁盘剩余空间不足（已保留安全余量）",
        ));
    }

    let mut id = crypto::random_b64(18);
    while state.uploads.contains_key(&id) {
        id = crypto::random_b64(18);
    }
    let temp_path = state.cfg.tmp_root.join(format!("{id}.part"));
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;

    let logical = if dir_clean.is_empty() {
        format!("/{}", request.name)
    } else {
        format!("/{dir_clean}/{}", request.name)
    };

    state.uploads.insert(
        id.clone(),
        Arc::new(UploadSession {
            owner: session.token.clone(),
            inner: Mutex::new(UploadInner {
                temp_path,
                final_path,
                logical_path: logical,
                declared: request.size,
                received: 0,
                next_seq: 0,
                file: Some(file),
            }),
            last_active: AtomicU64::new(unix_secs()),
        }),
    );

    encrypted_response(&session, &aad, json!({ "id": id }))
}

pub async fn upload_chunk(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    UrlPath((id, seq)): UrlPath<(String, u64)>,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let Some(upload) = state.uploads.get(&id).map(|entry| entry.value().clone()) else {
        return Err(ApiError::not_found("上传会话不存在或已过期"));
    };
    if upload.owner != session.token {
        return Err(ApiError::not_found("上传会话不存在或已过期"));
    }

    let raw = axum::body::to_bytes(body, CHUNK_BODY_LIMIT)
        .await
        .map_err(|_| ApiError::too_large())?;
    if raw.len() < 4 + 12 + 16 {
        return Err(ApiError::bad("上传分块格式错误"));
    }
    let ciphertext_len = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
    if !(16..=UPLOAD_CHUNK_MAX + 16).contains(&ciphertext_len) {
        return Err(ApiError::bad("上传分块长度非法"));
    }
    if raw.len() != 16 + ciphertext_len {
        return Err(ApiError::bad("上传分块长度不匹配"));
    }
    let iv = &raw[4..16];
    let ciphertext = &raw[16..];

    state.upload_limiter.wait(raw.len()).await;
    let cipher =
        Aes256Gcm::new_from_slice(&session.key).map_err(|_| ApiError::internal("会话密钥无效"))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(iv),
            Payload {
                msg: ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| ApiError::bad("上传分块解密失败"))?;

    if let Some(free) = free_space(&state.cfg.tmp_root) {
        if free
            < state
                .cfg
                .disk_reserve_bytes
                .saturating_add(plaintext.len() as u64)
        {
            return Err(ApiError::new(
                StatusCode::INSUFFICIENT_STORAGE,
                "磁盘剩余空间不足，已停止本次上传",
            ));
        }
    }

    let permit = state
        .upload_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("上传并发控制器已关闭"))?;

    let received = {
        let mut inner = upload.inner.lock().await;
        if inner.file.is_none() {
            return Err(ApiError::not_found("上传会话已结束"));
        }
        if seq != inner.next_seq {
            return Err(ApiError::conflict("上传分块顺序错误"));
        }
        if inner.received.saturating_add(plaintext.len() as u64) > inner.declared {
            return Err(ApiError::bad("上传数据超过声明大小"));
        }
        let file = inner.file.as_mut().expect("file checked above");
        file.write_all(&plaintext)
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
        inner.received += plaintext.len() as u64;
        inner.next_seq += 1;
        inner.received
    };
    drop(permit);
    upload.last_active.store(unix_secs(), Ordering::Relaxed);

    encrypted_response(&session, &aad, json!({ "received": received }))
}

async fn remove_temp(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

/// Desktop local mode keeps temp files in the app data directory, which can be
/// on another filesystem than the selected root. Fall back to copy+remove.
async fn move_file(source: &Path, target: &Path) -> io::Result<()> {
    match tokio::fs::rename(source, target).await {
        Ok(()) => Ok(()),
        Err(rename_error) => match tokio::fs::copy(source, target).await {
            Ok(_) => {
                let _ = tokio::fs::remove_file(source).await;
                Ok(())
            }
            Err(_) => Err(rename_error),
        },
    }
}

pub async fn upload_finish(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: UploadIdRequest = read_encrypted(&session, &aad, body).await?;

    let Some(upload) = state
        .uploads
        .get(&request.id)
        .map(|entry| entry.value().clone())
    else {
        return Err(ApiError::not_found("上传会话不存在或已过期"));
    };
    if upload.owner != session.token {
        return Err(ApiError::not_found("上传会话不存在或已过期"));
    }
    let Some((_, upload)) = state
        .uploads
        .remove_if(&request.id, |_, value| Arc::ptr_eq(value, &upload))
    else {
        return Err(ApiError::not_found("上传会话不存在或已过期"));
    };

    let mut inner = upload.inner.lock().await;
    let Some(file) = inner.file.take() else {
        drop(inner);
        return Err(ApiError::not_found("上传会话已结束"));
    };
    if inner.received != inner.declared {
        let temp = inner.temp_path.clone();
        drop(file);
        drop(inner);
        remove_temp(&temp).await;
        return Err(ApiError::bad("上传数据不完整，请重新上传"));
    }
    if let Err(err) = file.sync_all().await {
        let temp = inner.temp_path.clone();
        drop(file);
        drop(inner);
        remove_temp(&temp).await;
        return Err(ApiError::internal(err.to_string()));
    }
    drop(file);

    let temp_path = inner.temp_path.clone();
    let final_path = inner.final_path.clone();
    let logical_path = inner.logical_path.clone();
    let size = inner.received;
    drop(inner);

    if tokio::fs::symlink_metadata(&final_path).await.is_ok() {
        remove_temp(&temp_path).await;
        return Err(ApiError::conflict("同名文件或文件夹已存在"));
    }
    if let Err(err) = move_file(&temp_path, &final_path).await {
        remove_temp(&temp_path).await;
        return Err(ApiError::internal(err.to_string()));
    }
    invalidate_listings(&state);
    log(&format!(
        "上传完成 user={} path={} size={}",
        session.user, logical_path, size
    ));

    encrypted_response(
        &session,
        &aad,
        json!({ "ok": true, "path": logical_path, "size": size }),
    )
}

pub async fn upload_abort(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: UploadIdRequest = read_encrypted(&session, &aad, body).await?;

    let Some(upload) = state
        .uploads
        .get(&request.id)
        .map(|entry| entry.value().clone())
    else {
        return Err(ApiError::not_found("上传会话不存在"));
    };
    if upload.owner != session.token {
        return Err(ApiError::not_found("上传会话不存在"));
    }
    let Some((_, upload)) = state
        .uploads
        .remove_if(&request.id, |_, value| Arc::ptr_eq(value, &upload))
    else {
        return Err(ApiError::not_found("上传会话不存在"));
    };

    let mut inner = upload.inner.lock().await;
    let file = inner.file.take();
    let temp_path = inner.temp_path.clone();
    drop(inner);
    drop(file);
    remove_temp(&temp_path).await;
    encrypted_response(&session, &aad, json!({ "ok": true }))
}

pub fn clean_orphan_uploads(cfg: &crate::config::Config) {
    let Ok(reader) = std::fs::read_dir(&cfg.tmp_root) else {
        return;
    };
    let mut removed = 0usize;
    for entry in reader.flatten() {
        let path = entry.path();
        let is_part = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension == "part")
            .unwrap_or(false);
        if is_part && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log(&format!("清理了 {removed} 个未完成的上传临时文件"));
    }
}

pub async fn cleanup_stale_uploads(state: &Arc<AppState>) {
    let cutoff = unix_secs().saturating_sub(6 * 3600);
    let stale: Vec<(String, Arc<UploadSession>)> = state
        .uploads
        .iter()
        .filter(|entry| entry.value().last_active.load(Ordering::Relaxed) < cutoff)
        .map(|entry| (entry.key().clone(), entry.value().clone()))
        .collect();

    for (id, upload) in stale {
        let Some((_, upload)) = state
            .uploads
            .remove_if(&id, |_, value| Arc::ptr_eq(value, &upload))
        else {
            continue;
        };
        let mut inner = upload.inner.lock().await;
        let file = inner.file.take();
        let temp_path = inner.temp_path.clone();
        drop(inner);
        drop(file);
        remove_temp(&temp_path).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_cleaning_rejects_traversal() {
        assert_eq!(clean_rel("/a/b").unwrap(), "a/b");
        assert_eq!(clean_rel("a//./b").unwrap(), "a/b");
        assert!(clean_rel("../etc/passwd").is_err());
        assert!(clean_rel("a/../../b").is_err());
        assert_eq!(clean_rel("/").unwrap(), "");
    }

    #[test]
    fn file_names_are_validated() {
        assert!(validate_name("正常文件.txt").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn parent_split_works() {
        assert_eq!(split_parent("a/b.txt").unwrap(), ("a", "b.txt"));
        assert_eq!(split_parent("b.txt").unwrap(), ("", "b.txt"));
        assert!(split_parent("a/").is_err());
    }
}
