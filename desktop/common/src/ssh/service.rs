//! Loopback HTTP service for one SSH target.
//!
//! Its routes, request envelopes, encryption framing and JSON shapes are the
//! same as the local service (`agentgod_files::web`), so the companion proxy
//! and the whole frontend work without a special case.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex, RwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use axum::{
    body::Body,
    extract::{OriginalUri, State},
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use dashmap::DashMap;
use futures_util::stream::try_unfold;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, Mutex, Semaphore},
};

use agentgod_files::{
    api::{encrypted_response, read_encrypted},
    auth::AuthSession,
    crypto::{self, random_b64, random_key, seal_frame},
    error::{log, ApiError},
    files::rfc5987,
    preview::content_type_for,
    rate::RateLimiter,
    state::ReplayWindow,
};

use crate::config::SshTarget;
use crate::known_hosts::KnownHosts;
use crate::secrets::Secrets;

use super::connect::{connect, ConnectParams, HostKeyInfo, SshSession};
use super::fs::{clean_rel, validate_name, FsError, FsErrorKind, RemoteEntry, SftpFs};
use super::{tar, SshError};

const TOKEN: &str = "local";
const DOWNLOAD_CHUNK: usize = 64 * 1024;
const UPLOAD_CHUNK_MAX: usize = 4 * 1024 * 1024;
const CHUNK_BODY_LIMIT: usize = 4 + 12 + UPLOAD_CHUNK_MAX + 16 + 64;
const LIST_PAGE_LIMIT: usize = 1_000;
const LISTING_TTL: Duration = Duration::from_secs(15);
const MAX_ACTIVE_UPLOADS: usize = 32;
const MAX_ENSURE_DIRS: usize = 1_024;
const ARCHIVE_CHUNK: usize = 64 * 1024;

struct Connected {
    session: SshSession,
    fs: Arc<SftpFs>,
}

struct CachedListing {
    entries: Arc<Vec<Entry>>,
    truncated: bool,
    built_at: Instant,
}

#[derive(Clone, Serialize)]
struct Entry {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    modified: u64,
}

struct UploadInner {
    temp_path: PathBuf,
    remote_clean: String,
    declared: u64,
    received: u64,
    next_seq: u64,
    file: Option<tokio::fs::File>,
}

struct UploadSession {
    owner: String,
    inner: Mutex<UploadInner>,
    last_active: AtomicU64,
}

pub struct SshState {
    target: SshTarget,
    key: [u8; 32],
    replay: Arc<StdMutex<ReplayWindow>>,
    session: Mutex<Option<Arc<Connected>>>,
    root: RwLock<String>,
    home: RwLock<String>,
    listings: DashMap<String, CachedListing>,
    uploads: DashMap<String, Arc<UploadSession>>,
    upload_sem: Arc<Semaphore>,
    download_sem: Arc<Semaphore>,
    preview_sem: Arc<Semaphore>,
    upload_limiter: Arc<RateLimiter>,
    download_limiter: Arc<RateLimiter>,
    secrets: Arc<dyn Secrets>,
    known_hosts: Arc<StdMutex<KnownHosts>>,
    pending_host_key: StdMutex<Option<HostKeyInfo>>,
    tmp_dir: PathBuf,
}

impl SshState {
    pub fn new(
        target: SshTarget,
        data_dir: &std::path::Path,
        secrets: Arc<dyn Secrets>,
        known_hosts: Arc<StdMutex<KnownHosts>>,
    ) -> Self {
        let tmp_dir = data_dir.join("ssh-tmp").join(&target.id);
        let _ = std::fs::create_dir_all(&tmp_dir);
        Self {
            target,
            key: random_key(),
            replay: Arc::new(StdMutex::new(ReplayWindow::new(Duration::from_secs(
                3650 * 24 * 3600,
            )))),
            session: Mutex::new(None),
            root: RwLock::new("/".to_string()),
            home: RwLock::new("/".to_string()),
            listings: DashMap::new(),
            uploads: DashMap::new(),
            upload_sem: Arc::new(Semaphore::new(2)),
            download_sem: Arc::new(Semaphore::new(2)),
            preview_sem: Arc::new(Semaphore::new(4)),
            upload_limiter: Arc::new(RateLimiter::new(0.0, 0.0)),
            download_limiter: Arc::new(RateLimiter::new(0.0, 0.0)),
            secrets,
            known_hosts,
            pending_host_key: StdMutex::new(None),
            tmp_dir,
        }
    }

    pub fn target(&self) -> &SshTarget {
        &self.target
    }

    pub fn root(&self) -> String {
        self.root.read().expect("root lock").clone()
    }

    fn auth_session(&self) -> AuthSession {
        AuthSession {
            token: TOKEN.to_string(),
            key: self.key,
            user: self.target.username.clone(),
            replay: self.replay.clone(),
        }
    }

    fn set_root(&self, root: String) {
        *self.root.write().expect("root lock") = root;
        self.listings.clear();
    }

    fn invalidate_listings(&self) {
        self.listings.clear();
    }

    /// Connects on demand. The lock is held across the handshake so concurrent
    /// requests share one attempt instead of racing several SSH connections.
    async fn connected(&self) -> Result<Arc<Connected>, SshError> {
        let mut guard = self.session.lock().await;
        if let Some(connected) = guard.as_ref() {
            if !connected.session.is_closed() {
                return Ok(connected.clone());
            }
            // The transport died (server restart, network drop); drop the
            // cached SFTP session and handshake again with the stored pin.
            log(&format!(
                "SSH 连接已断开，正在重连 {}:{}",
                self.target.host, self.target.port
            ));
            *guard = None;
        }
        let connected = Arc::new(self.connect_once().await?);
        *guard = Some(connected.clone());
        Ok(connected)
    }

    async fn connect_once(&self) -> Result<Connected, SshError> {
        let password = match self.target.auth {
            crate::config::SshAuth::Password => self
                .secrets
                .get(&crate::secrets::account(&self.target.id, "password"))
                .map_err(|err| SshError::internal(err.to_string()))?,
            crate::config::SshAuth::Key => None,
        };
        let passphrase = match self.target.auth {
            crate::config::SshAuth::Key => self
                .secrets
                .get(&crate::secrets::account(&self.target.id, "passphrase"))
                .map_err(|err| SshError::internal(err.to_string()))?,
            crate::config::SshAuth::Password => None,
        };
        let pinned = self
            .known_hosts
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries()
            .iter()
            .find(|entry| entry.host == self.target.host && entry.port == self.target.port)
            .map(|entry| HostKeyInfo {
                host: entry.host.clone(),
                port: entry.port,
                algorithm: entry.algorithm.clone(),
                key: entry.key.clone(),
                fingerprint: entry.fingerprint.clone(),
            });

        let result = connect(ConnectParams {
            host: self.target.host.clone(),
            port: self.target.port,
            username: self.target.username.clone(),
            auth: self.target.auth,
            key_path: self.target.key_path.clone(),
            password,
            passphrase,
            pinned,
        })
        .await;

        let session = match result {
            Ok(session) => session,
            Err(err) => {
                if err.code == "host_key_unknown" {
                    *self
                        .pending_host_key
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = err.host_key.clone();
                }
                return Err(err);
            }
        };
        let sftp = session.sftp.clone();
        let requested = self
            .target
            .root
            .clone()
            .filter(|root| !root.trim().is_empty())
            .unwrap_or_else(|| ".".to_string());
        let canonical = sftp.canonicalize(&requested).await.map_err(|_| {
            SshError::new(
                "ssh_connect_failed",
                format!("远程目录不存在或不可访问：{requested}"),
            )
        })?;
        let home = sftp
            .canonicalize(".")
            .await
            .unwrap_or_else(|_| canonical.clone());
        *self.root.write().expect("root lock") = canonical.clone();
        *self.home.write().expect("home lock") = home;
        self.listings.clear();
        if let Ok(mut store) = self.known_hosts.lock() {
            store.touch(&self.target.host, self.target.port);
        }
        Ok(Connected {
            fs: Arc::new(SftpFs::new(sftp, canonical)),
            session,
        })
    }

    async fn fs(&self) -> Result<Arc<SftpFs>, ApiError> {
        Ok(self
            .connected()
            .await
            .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, err.message))?
            .fs
            .clone())
    }
}

fn map_fs(err: FsError) -> ApiError {
    let status = match err.kind {
        FsErrorKind::NotFound => StatusCode::NOT_FOUND,
        FsErrorKind::Conflict => StatusCode::CONFLICT,
        FsErrorKind::Forbidden => StatusCode::FORBIDDEN,
        FsErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    if err.kind == FsErrorKind::Internal {
        log(&format!("SSH 操作失败: {}", err.message));
        return ApiError::new(status, "SSH 操作失败，请查看日志");
    }
    ApiError::new(status, err.message)
}

pub fn router(state: Arc<SshState>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/app/info", get(info))
        .route("/app/local/root", post(set_root))
        .route("/app/ssh/root", post(set_root))
        .route("/app/ssh/status", get(status))
        .route("/app/ssh/test", post(test_connection))
        .route("/app/ssh/trust", post(trust_host_key))
        .route("/app/ssh/forget", post(forget_host_key))
        .route("/api/local-login", get(local_login))
        .route("/api/list", post(list))
        .route("/api/mkdir", post(mkdir))
        .route("/api/ensure_dirs", post(ensure_dirs))
        .route("/api/rename", post(rename))
        .route("/api/delete", post(delete))
        .route("/api/download", post(download))
        .route("/api/download_folder", post(download_folder))
        .route("/api/preview/stream", post(preview))
        .route("/api/upload/init", post(upload_init))
        .route("/api/upload/chunk/{id}/{seq}", put(upload_chunk))
        .route("/api/upload/finish", post(upload_finish))
        .route("/api/upload/abort", post(upload_abort))
        .layer(middleware::from_fn(loopback_guard))
        .with_state(state)
}

pub struct SshService {
    pub base_url: String,
    state: Arc<SshState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SshService {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SshService {
    pub fn state(&self) -> Arc<SshState> {
        self.state.clone()
    }
}

pub async fn start(
    target: SshTarget,
    data_dir: &std::path::Path,
    secrets: Arc<dyn Secrets>,
    known_hosts: Arc<StdMutex<KnownHosts>>,
) -> anyhow::Result<SshService> {
    let state = Arc::new(SshState::new(target, data_dir, secrets, known_hosts));
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            log(&format!("SSH 服务异常退出: {err}"));
        }
    });
    Ok(SshService {
        base_url: format!("http://{addr}"),
        state,
        task,
    })
}

async fn loopback_guard(request: axum::extract::Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !agentgod_files::web::is_loopback_host(host) {
        return (StatusCode::MISDIRECTED_REQUEST, "invalid host").into_response();
    }
    next.run(request).await
}

async fn health() -> Response {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "ok").into_response()
}

async fn info(State(state): State<Arc<SshState>>) -> Response {
    let root = state.root();
    let home = state.home.read().expect("home lock").clone();
    let mut roots = vec![json!({ "name": "根目录", "path": "/" })];
    if home != "/" {
        roots.push(json!({ "name": "主目录", "path": home }));
    }
    if !roots.iter().any(|entry| entry["path"] == root) {
        roots.push(json!({ "name": "当前目录", "path": root }));
    }
    Json(json!({
        "mode": "ssh",
        "version": agentgod_files::VERSION,
        "platform": std::env::consts::OS,
        "root": root,
        "roots": roots,
        "host": state.target.host,
        "port": state.target.port,
        "user": state.target.username,
    }))
    .into_response()
}

fn ssh_failure_response(err: &SshError) -> Response {
    let status = match err.code.as_str() {
        "host_key_unknown" | "host_key_mismatch" => StatusCode::CONFLICT,
        "ssh_auth_failed" => StatusCode::UNAUTHORIZED,
        _ => StatusCode::BAD_GATEWAY,
    };
    (
        status,
        Json(serde_json::to_value(err).unwrap_or_else(|_| json!({}))),
    )
        .into_response()
}

async fn local_login(State(state): State<Arc<SshState>>) -> Response {
    match state.connected().await {
        Ok(_) => {
            let mut response = (
                StatusCode::OK,
                Json(json!({
                    "token": TOKEN,
                    "key": STANDARD.encode(state.key),
                    "user": state.target.username,
                })),
            )
                .into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(err) => ssh_failure_response(&err),
    }
}

async fn status(State(state): State<Arc<SshState>>) -> Response {
    let connected = state.session.lock().await.is_some();
    Json(json!({
        "connected": connected,
        "host": state.target.host,
        "port": state.target.port,
        "user": state.target.username,
        "root": state.root(),
    }))
    .into_response()
}

async fn test_connection(State(state): State<Arc<SshState>>) -> Response {
    match state.connected().await {
        Ok(_) => Json(json!({ "ok": true, "root": state.root() })).into_response(),
        Err(err) => ssh_failure_response(&err),
    }
}

#[derive(Deserialize)]
struct RootRequest {
    path: String,
}

async fn set_root(
    State(state): State<Arc<SshState>>,
    Json(request): Json<RootRequest>,
) -> Response {
    let fs = match state.fs().await {
        Ok(fs) => fs,
        Err(err) => return err.into_response(),
    };
    let path = request.path.trim().to_string();
    if !path.starts_with('/') {
        return ApiError::bad("必须是绝对路径").into_response();
    }
    match fs.realpath(&path).await {
        Ok(canonical) if fs.is_dir(&canonical).await => {
            state.set_root(canonical.clone());
            Json(json!({ "ok": true, "root": canonical })).into_response()
        }
        _ => ApiError::bad("路径不存在或不是文件夹").into_response(),
    }
}

#[derive(Deserialize)]
struct TrustRequest {
    fingerprint: String,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    accept: Option<bool>,
}

fn remember_host_key(state: &SshState, pending: &HostKeyInfo) -> std::io::Result<()> {
    let mut store = state.known_hosts.lock().unwrap_or_else(|e| e.into_inner());
    let entry = crate::known_hosts::host_key(
        &pending.host,
        pending.port,
        &pending.algorithm,
        &pending.key,
        &pending.fingerprint,
    );
    store.trust(entry)
}

fn forget_stored_host(state: &SshState, host: &str, port: u16) -> std::io::Result<usize> {
    let mut store = state.known_hosts.lock().unwrap_or_else(|e| e.into_inner());
    store.forget(host, port)
}

async fn trust_host_key(
    State(state): State<Arc<SshState>>,
    Json(request): Json<TrustRequest>,
) -> Response {
    if matches!(request.accept, Some(false)) {
        *state
            .pending_host_key
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        return Json(json!({ "ok": true, "trusted": false })).into_response();
    }
    let pending = state
        .pending_host_key
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let Some(pending) = pending else {
        return ApiError::conflict("没有待确认的主机指纹，请重新连接").into_response();
    };
    if !constant_time_eq(&pending.fingerprint, &request.fingerprint) {
        return ApiError::bad("指纹不匹配，请重新连接后再确认").into_response();
    }
    if let Some(host) = request.host.as_deref() {
        if host != pending.host {
            return ApiError::bad("主机不匹配").into_response();
        }
    }
    if let Some(port) = request.port {
        if port != pending.port {
            return ApiError::bad("端口不匹配").into_response();
        }
    }
    if let Err(err) = remember_host_key(&state, &pending) {
        return ApiError::internal(err.to_string()).into_response();
    }
    *state
        .pending_host_key
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    // The next login retries the handshake with the pin in place.
    let mut session = state.session.lock().await;
    *session = None;
    Json(json!({ "ok": true, "trusted": true, "fingerprint": pending.fingerprint })).into_response()
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[derive(Deserialize)]
struct ForgetRequest {
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
}

async fn forget_host_key(
    State(state): State<Arc<SshState>>,
    Json(request): Json<ForgetRequest>,
) -> Response {
    let host = request.host.unwrap_or_else(|| state.target.host.clone());
    let port = request.port.unwrap_or(state.target.port);
    match forget_stored_host(&state, &host, port) {
        Ok(removed) => {
            *state.session.lock().await = None;
            Json(json!({ "ok": true, "removed": removed })).into_response()
        }
        Err(err) => ApiError::internal(err.to_string()).into_response(),
    }
}

// ---------- File API ----------

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

async fn list(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: ListRequest = read_encrypted(&session, &aad, body).await?;
    let fs = state.fs().await?;
    let clean = clean_rel(&request.path).map_err(map_fs)?;
    let (entries, truncated) = load_listing(&state, &fs, &clean).await?;
    let total = entries.len();
    let limit = request.limit.clamp(1, LIST_PAGE_LIMIT);
    let start = request.offset.min(total);
    let end = start.saturating_add(limit).min(total);
    let page = entries[start..end].to_vec();
    let free_space = fs.available_space(&clean).await;
    let logical = if clean.is_empty() {
        "/".to_string()
    } else {
        format!("/{clean}")
    };
    encrypted_response(
        &session,
        &aad,
        json!({
            "path": logical,
            "entries": page,
            "total": total,
            "offset": start,
            "limit": limit,
            "has_more": end < total,
            "truncated": truncated,
            "free_space": free_space,
        }),
    )
}

async fn load_listing(
    state: &SshState,
    fs: &SftpFs,
    clean: &str,
) -> Result<(Arc<Vec<Entry>>, bool), ApiError> {
    let key = fs.join(clean);
    if let Some(cached) = state.listings.get(&key) {
        if cached.built_at.elapsed() < LISTING_TTL {
            return Ok((cached.entries.clone(), cached.truncated));
        }
    }
    let (entries, truncated) = fs.list(clean).await.map_err(map_fs)?;
    let entries = Arc::new(
        entries
            .into_iter()
            .map(|entry: RemoteEntry| Entry {
                name: entry.name,
                is_dir: entry.is_dir,
                is_symlink: entry.is_symlink,
                size: entry.size,
                modified: entry.modified,
            })
            .collect::<Vec<_>>(),
    );
    state.listings.insert(
        key,
        CachedListing {
            entries: entries.clone(),
            truncated,
            built_at: Instant::now(),
        },
    );
    Ok((entries, truncated))
}

async fn mkdir(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: MkdirRequest = read_encrypted(&session, &aad, body).await?;
    let fs = state.fs().await?;
    let clean = clean_rel(&request.path).map_err(map_fs)?;
    if clean.is_empty() {
        return Err(ApiError::bad("文件夹名称不能为空"));
    }
    if fs.stat(&clean).await.map_err(map_fs)?.is_some() {
        return Err(ApiError::conflict("同名文件或文件夹已存在"));
    }
    fs.ensure_dir(&clean).await.map_err(map_fs)?;
    state.invalidate_listings();
    let logical = format!("/{clean}");
    encrypted_response(&session, &aad, json!({ "ok": true, "path": logical }))
}

async fn ensure_dirs(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: EnsureDirsRequest = read_encrypted(&session, &aad, body).await?;
    if request.paths.len() > MAX_ENSURE_DIRS {
        return Err(ApiError::bad("一次创建的目录过多"));
    }
    let fs = state.fs().await?;
    for path in &request.paths {
        let clean = clean_rel(path).map_err(map_fs)?;
        if clean.is_empty() {
            continue;
        }
        match fs.stat(&clean).await.map_err(map_fs)? {
            Some(attr) if attr.is_dir => continue,
            Some(_) => return Err(ApiError::conflict("路径中存在同名文件")),
            None => {
                fs.ensure_dir(&clean).await.map_err(map_fs)?;
            }
        }
    }
    state.invalidate_listings();
    encrypted_response(&session, &aad, json!({ "ok": true }))
}

async fn rename(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: RenameRequest = read_encrypted(&session, &aad, body).await?;
    let fs = state.fs().await?;
    let clean = clean_rel(&request.path).map_err(map_fs)?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能重命名根目录"));
    }
    validate_name(&request.new_name).map_err(map_fs)?;
    fs.rename(&clean, &request.new_name).await.map_err(map_fs)?;
    state.invalidate_listings();
    let parent = match clean.rfind('/') {
        Some(index) => &clean[..index],
        None => "",
    };
    let logical = if parent.is_empty() {
        format!("/{}", request.new_name)
    } else {
        format!("/{parent}/{}", request.new_name)
    };
    encrypted_response(&session, &aad, json!({ "ok": true, "path": logical }))
}

async fn delete(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: DeleteRequest = read_encrypted(&session, &aad, body).await?;
    let fs = state.fs().await?;
    let clean = clean_rel(&request.path).map_err(map_fs)?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能删除根目录"));
    }
    fs.delete(&clean).await.map_err(map_fs)?;
    state.invalidate_listings();
    encrypted_response(&session, &aad, json!({ "ok": true }))
}

async fn download(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: DownloadRequest = read_encrypted(&session, &aad, body).await?;
    let fs = state.fs().await?;
    let clean = clean_rel(&request.path).map_err(map_fs)?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能下载根目录"));
    }
    let name = clean.rsplit('/').next().unwrap_or("download").to_string();
    let (file, size) = fs.open_read(&clean).await.map_err(map_fs)?;
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
            return Ok::<_, std::io::Error>(None);
        }
        buffer.truncate(read);
        stream.limiter.wait(4 + 12 + buffer.len() + 16).await;
        let aad = format!("dl:{}", stream.seq);
        let frame = seal_frame(&stream.cipher, &aad, &buffer)
            .map_err(|_| std::io::Error::other("下载分块加密失败"))?;
        stream.seq += 1;
        Ok(Some((axum::body::Bytes::from(frame), stream)))
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

struct DownloadStream {
    file: russh_sftp::client::fs::File,
    cipher: Aes256Gcm,
    seq: u64,
    limiter: Arc<RateLimiter>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

struct ArchiveStream {
    rx: mpsc::Receiver<ArchiveMessage>,
    cipher: Aes256Gcm,
    seq: u64,
    limiter: Arc<RateLimiter>,
    ended: bool,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

enum ArchiveMessage {
    Data(Vec<u8>),
    End,
}

async fn download_folder(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: DownloadRequest = read_encrypted(&session, &aad, body).await?;
    let fs = state.fs().await?;
    let clean = clean_rel(&request.path).map_err(map_fs)?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能打包根目录"));
    }
    let attr = fs
        .stat(&clean)
        .await
        .map_err(map_fs)?
        .ok_or_else(|| ApiError::not_found("文件夹不存在"))?;
    if !attr.is_dir {
        return Err(ApiError::bad("该路径不是文件夹"));
    }
    let folder_name = clean.rsplit('/').next().unwrap_or("archive").to_string();
    let permit = state
        .download_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("下载并发控制器已关闭"))?;
    let cipher =
        Aes256Gcm::new_from_slice(&session.key).map_err(|_| ApiError::internal("会话密钥无效"))?;
    let rx = spawn_archive(fs, clean, folder_name.clone());
    let stream_state = ArchiveStream {
        rx,
        cipher,
        seq: 0,
        limiter: state.download_limiter.clone(),
        ended: false,
        _permit: permit,
    };
    let stream = try_unfold(stream_state, |mut stream| async move {
        if stream.ended {
            return Ok::<_, std::io::Error>(None);
        }
        match stream.rx.recv().await {
            Some(ArchiveMessage::Data(data)) => {
                stream.limiter.wait(4 + 12 + data.len() + 16).await;
                let aad = format!("dl:{}", stream.seq);
                let frame = seal_frame(&stream.cipher, &aad, &data)
                    .map_err(|_| std::io::Error::other("归档分块加密失败"))?;
                stream.seq += 1;
                Ok(Some((axum::body::Bytes::from(frame), stream)))
            }
            Some(ArchiveMessage::End) => {
                stream
                    .limiter
                    .wait(4 + 12 + tar::END_MARKER.len() + 16)
                    .await;
                let aad = format!("dl:{}", stream.seq);
                let frame = seal_frame(&stream.cipher, &aad, tar::END_MARKER.as_bytes())
                    .map_err(|_| std::io::Error::other("归档结束标记加密失败"))?;
                stream.seq += 1;
                stream.ended = true;
                Ok(Some((axum::body::Bytes::from(frame), stream)))
            }
            None => Err(std::io::Error::other("目录归档流异常中断")),
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

fn spawn_archive(
    fs: Arc<SftpFs>,
    clean: String,
    _folder_name: String,
) -> mpsc::Receiver<ArchiveMessage> {
    let (tx, rx) = mpsc::channel::<ArchiveMessage>(8);
    tokio::spawn(async move {
        let outcome = build_archive(fs, clean, &tx).await;
        if let Err(err) = outcome {
            log(&format!("SSH 目录打包失败: {err}"));
            return;
        }
        let _ = tx.send(ArchiveMessage::End).await;
    });
    rx
}

async fn build_archive(
    fs: Arc<SftpFs>,
    clean: String,
    tx: &mpsc::Sender<ArchiveMessage>,
) -> Result<(), FsError> {
    let mut pending = Vec::new();
    let base_name = clean.rsplit('/').next().unwrap_or("archive").to_string();
    let root_attr = fs
        .stat(&clean)
        .await?
        .ok_or_else(|| FsError::not_found("文件夹不存在"))?;
    let mut queue = vec![(clean, base_name, root_attr.modified)];
    let mut files = Vec::new();
    while let Some((path, name, mtime)) = queue.pop() {
        tar::entry_header(&name, 0, true, mtime, &mut pending);
        flush_buffer(&mut pending, tx).await?;
        let (entries, _) = fs.list(&path).await?;
        for entry in entries {
            let child_clean = if path.is_empty() {
                entry.name.clone()
            } else {
                format!("{path}/{}", entry.name)
            };
            let child_name = format!("{name}/{}", entry.name);
            // Symlinks are skipped, matching the local archive writer.
            if entry.is_symlink {
                continue;
            }
            if entry.is_dir {
                queue.push((child_clean, child_name, entry.modified));
                continue;
            }
            files.push((child_clean, child_name, entry.size, entry.modified));
        }
    }
    for (path, name, size, mtime) in files {
        tar::entry_header(&name, size, false, mtime, &mut pending);
        flush_buffer(&mut pending, tx).await?;
        let (mut file, _) = fs.open_read(&path).await?;
        let mut buffer = vec![0u8; ARCHIVE_CHUNK];
        let mut remaining = size;
        while remaining > 0 {
            let read = file
                .read(&mut buffer)
                .await
                .map_err(|err| FsError::internal(format!("读取远程文件失败：{err}")))?;
            if read == 0 {
                break;
            }
            pending.extend_from_slice(&buffer[..read]);
            remaining = remaining.saturating_sub(read as u64);
            flush_buffer(&mut pending, tx).await?;
        }
        let pad = tar::padding(size);
        if pad > 0 {
            pending.extend(std::iter::repeat_n(0u8, pad));
            flush_buffer(&mut pending, tx).await?;
        }
    }
    tar::finish(&mut pending);
    flush_all(&mut pending, tx).await?;
    Ok(())
}

async fn flush_all(
    pending: &mut Vec<u8>,
    tx: &mpsc::Sender<ArchiveMessage>,
) -> Result<(), FsError> {
    if pending.is_empty() {
        return Ok(());
    }
    let data = std::mem::take(pending);
    tx.send(ArchiveMessage::Data(data))
        .await
        .map_err(|_| FsError::internal("归档流已关闭"))
}

async fn flush_buffer(
    pending: &mut Vec<u8>,
    tx: &mpsc::Sender<ArchiveMessage>,
) -> Result<(), FsError> {
    if pending.len() < ARCHIVE_CHUNK {
        return Ok(());
    }
    let data = std::mem::take(pending);
    tx.send(ArchiveMessage::Data(data))
        .await
        .map_err(|_| FsError::internal("归档流已关闭"))
}

async fn preview(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: DownloadRequest = read_encrypted(&session, &aad, body).await?;
    let fs = state.fs().await?;
    let clean = clean_rel(&request.path).map_err(map_fs)?;
    if clean.is_empty() {
        return Err(ApiError::bad("不能预览根目录"));
    }
    let name = clean.rsplit('/').next().unwrap_or("preview").to_string();
    let (file, size) = fs.open_read(&clean).await.map_err(map_fs)?;
    let content_type = content_type_for(&name);
    let permit = state
        .preview_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("预览并发控制器已关闭"))?;
    let cipher =
        Aes256Gcm::new_from_slice(&session.key).map_err(|_| ApiError::internal("会话密钥无效"))?;
    let stream_id = random_b64(18);
    let stream_state = PreviewStream {
        file,
        cipher,
        seq: 0,
        stream_id: stream_id.clone(),
        limiter: state.download_limiter.clone(),
        _permit: permit,
    };
    let stream = try_unfold(stream_state, |mut stream| async move {
        let mut buffer = vec![0u8; DOWNLOAD_CHUNK];
        let read = stream.file.read(&mut buffer).await?;
        if read == 0 {
            return Ok::<_, std::io::Error>(None);
        }
        buffer.truncate(read);
        stream.limiter.wait(4 + 12 + buffer.len() + 16).await;
        let aad = format!("pv:{}:{}", stream.stream_id, stream.seq);
        let frame = seal_frame(&stream.cipher, &aad, &buffer)
            .map_err(|_| std::io::Error::other("预览分块加密失败"))?;
        stream.seq += 1;
        Ok(Some((axum::body::Bytes::from(frame), stream)))
    });
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
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
        HeaderValue::from_str(&DOWNLOAD_CHUNK.to_string())
            .map_err(|_| ApiError::internal("构造分块大小响应头失败"))?,
    );
    headers.insert(
        HeaderName::from_static("x-preview-content-type"),
        HeaderValue::from_static(content_type),
    );
    Ok(response)
}

struct PreviewStream {
    file: russh_sftp::client::fs::File,
    cipher: Aes256Gcm,
    seq: u64,
    stream_id: String,
    limiter: Arc<RateLimiter>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

// ---------- Upload ----------

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

async fn upload_init(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
    let aad = crypto::request_aad(&method, &uri);
    let request: UploadInitRequest = read_encrypted(&session, &aad, body).await?;
    if state.uploads.len() >= MAX_ACTIVE_UPLOADS {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "同时进行的上传任务过多，请稍后重试",
        ));
    }
    validate_name(&request.name).map_err(map_fs)?;
    let dir_clean = clean_rel(&request.dir).map_err(map_fs)?;
    let fs = state.fs().await?;
    if dir_clean.is_empty() {
        // The root always exists.
    } else if fs
        .stat(&dir_clean)
        .await
        .map_err(map_fs)?
        .map(|attr| attr.is_dir)
        != Some(true)
    {
        fs.ensure_dir(&dir_clean).await.map_err(map_fs)?;
    }
    let remote_clean = if dir_clean.is_empty() {
        request.name.clone()
    } else {
        format!("{dir_clean}/{}", request.name)
    };
    if fs.stat(&remote_clean).await.map_err(map_fs)?.is_some() {
        return Err(ApiError::conflict("同名文件或文件夹已存在"));
    }
    let id = random_b64(18);
    let temp_path = state.tmp_dir.join(format!("{id}.part"));
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;
    state.uploads.insert(
        id.clone(),
        Arc::new(UploadSession {
            owner: session.token.clone(),
            inner: Mutex::new(UploadInner {
                temp_path,
                remote_clean,
                declared: request.size,
                received: 0,
                next_seq: 0,
                file: Some(file),
            }),
            last_active: AtomicU64::new(unix_secs()),
        }),
    );
    state.invalidate_listings();
    encrypted_response(&session, &aad, json!({ "id": id }))
}

async fn upload_chunk(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    axum::extract::Path((id, seq)): axum::extract::Path<(String, u64)>,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
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

async fn upload_finish(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
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
        return Err(ApiError::not_found("上传会话已结束"));
    };
    if inner.received != inner.declared {
        let temp = inner.temp_path.clone();
        drop(file);
        drop(inner);
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(ApiError::bad("上传数据不完整，请重新上传"));
    }
    if let Err(err) = file.sync_all().await {
        let temp = inner.temp_path.clone();
        drop(file);
        drop(inner);
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(ApiError::internal(err.to_string()));
    }
    drop(file);
    let temp_path = inner.temp_path.clone();
    let remote_clean = inner.remote_clean.clone();
    let size = inner.received;
    drop(inner);

    let fs = state.fs().await?;
    if fs.stat(&remote_clean).await.map_err(map_fs)?.is_some() {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::conflict("同名文件或文件夹已存在"));
    }
    let uploaded = fs.upload(&temp_path, &remote_clean).await.map_err(map_fs);
    let _ = tokio::fs::remove_file(&temp_path).await;
    uploaded?;
    state.invalidate_listings();
    let logical = format!("/{remote_clean}");
    log(&format!(
        "SSH 上传完成 target={} path={} size={}",
        state.target.id, logical, size
    ));
    encrypted_response(
        &session,
        &aad,
        json!({ "ok": true, "path": logical, "size": size }),
    )
}

async fn upload_abort(
    State(state): State<Arc<SshState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let session = state.auth_session();
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
    inner.file.take();
    let temp = inner.temp_path.clone();
    drop(inner);
    let _ = tokio::fs::remove_file(&temp).await;
    encrypted_response(&session, &aad, json!({ "ok": true }))
}

/// Removes staging files left behind by a previous run.
pub fn clean_orphan_uploads(tmp_dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(tmp_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "part")
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
