//! Shared runtime for the desktop app.
//!
//! The app hosts a tiny loopback HTTP server. It serves the same web UI as the
//! standalone server and routes API, preview and asset requests to the selected
//! target: the embedded local file service, a remote AgentGod Files server, or
//! a per-target SSH/SFTP service. Because the webview loads
//! `http://127.0.0.1:<port>/`, the existing same-origin frontend works
//! unchanged and streaming is preserved by proxying response bodies with
//! backpressure.

pub mod config;
pub mod known_hosts;
pub mod secrets;
pub mod ssh;
pub mod update;

pub use config::{HttpTarget, SshAuth, SshTarget, TargetEntry, TargetsConfig};
pub use known_hosts::KnownHosts;
pub use secrets::{KeyringSecrets, MemorySecrets, Secrets};
pub use update::{AutoInstaller, InstallStatus, NoopInstaller, UpdateHub, UpdateSource};

use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock as StdRwLock},
    time::Duration,
};

use anyhow::Context;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use agentgod_files::{
    config::Config,
    server::{start_http, HttpServer},
};

use ssh::service::SshService;

const FORWARD_REQUEST_HEADERS: &[&str] = &[
    "authorization",
    "content-type",
    "range",
    "if-range",
    "accept",
];

const FORWARD_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "content-range",
    "accept-ranges",
    "content-disposition",
    "cache-control",
    "x-file-size",
    "x-chunk-size",
    "x-stream-id",
    "x-preview-content-type",
    "x-encrypted",
    "x-encrypted-end",
    "x-content-type-options",
    "content-security-policy",
    "x-frame-options",
    "referrer-policy",
    "cross-origin-resource-policy",
    "cross-origin-opener-policy",
    "www-authenticate",
];

struct FrontState {
    config: StdRwLock<TargetsConfig>,
    data_dir: PathBuf,
    local_url: Option<String>,
    update: Arc<UpdateHub>,
    secrets: Arc<dyn Secrets>,
    known_hosts: Arc<Mutex<KnownHosts>>,
    ssh: AsyncMutex<HashMap<String, SshService>>,
    clients: Mutex<HashMap<String, reqwest::Client>>,
}

impl FrontState {
    fn client(&self, key: &str, allow_invalid_certs: bool) -> reqwest::Client {
        let mut clients = self.clients.lock().expect("client cache poisoned");
        clients
            .entry(format!("{key}:{allow_invalid_certs}"))
            .or_insert_with(|| {
                reqwest::Client::builder()
                    .danger_accept_invalid_certs(allow_invalid_certs)
                    .connect_timeout(Duration::from_secs(15))
                    .build()
                    .expect("failed to build HTTP client")
            })
            .clone()
    }

    fn save(&self) {
        let _ = config::save(&self.data_dir, &self.config.read().expect("config lock"));
    }
}

pub struct Runtime {
    pub url: String,
    pub local: Option<HttpServer>,
    _front: Arc<FrontState>,
}

pub fn data_dir() -> PathBuf {
    // Tests and portable installs can redirect the whole profile.
    if let Some(dir) = std::env::var_os("AGENTGOD_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("AgentGodFiles")
}

/// Headless entry point (used by the `dev_app` binary).
pub async fn start() -> anyhow::Result<Runtime> {
    start_with_updater(Arc::new(NoopInstaller)).await
}

/// Tauri shells pass an updater implementation; the companion exposes a small
/// same-origin HTTP API so the webview never needs a JS plugin or capability.
pub async fn start_with_updater(installer: Arc<dyn AutoInstaller>) -> anyhow::Result<Runtime> {
    let data_dir = data_dir();
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("无法创建数据目录: {}", data_dir.display()))?;

    // The app is local-first: the machine's filesystem is always available,
    // and remote targets are additional workspace tabs.
    let cfg = local_config(&data_dir)?;
    let local = Some(start_http(Arc::new(cfg)).await?);
    let local_url = local
        .as_ref()
        .map(|server| format!("http://{}", server.addr));

    let (mut config, notes) = config::load_or_migrate(&data_dir);
    for note in notes {
        agentgod_files::error::log(&note);
    }
    config.normalize_active(local_url.is_some());
    // Always open on this machine's files; saved targets stay one dropdown
    // away and are never the startup target.
    if local_url.is_some() {
        config.active = Some("local".to_string());
    }
    config::save(&data_dir, &config)?;

    let secrets: Arc<dyn Secrets> = if std::env::var("AGENTGOD_MEMORY_SECRETS").is_ok() {
        Arc::new(MemorySecrets::default())
    } else {
        Arc::new(KeyringSecrets::default())
    };
    let known_hosts = Arc::new(Mutex::new(KnownHosts::load(&data_dir)));

    let update_source = UpdateSource::load(&data_dir);
    let front = Arc::new(FrontState {
        config: StdRwLock::new(config),
        data_dir: data_dir.clone(),
        local_url,
        update: UpdateHub::new(installer, update_source, data_dir.clone()),
        secrets,
        known_hosts,
        ssh: AsyncMutex::new(HashMap::new()),
        clients: Mutex::new(HashMap::new()),
    });

    // Prepare SSH targets in the background: a slow or unreachable host must
    // never delay the window.
    let boot = front.clone();
    tokio::spawn(async move {
        let targets: Vec<SshTarget> = boot
            .config
            .read()
            .expect("config lock")
            .targets
            .iter()
            .filter_map(|target| match target {
                TargetEntry::Ssh(ssh) => Some(ssh.clone()),
                _ => None,
            })
            .collect();
        for target in targets {
            if let Err(err) = ensure_ssh(&boot, &target).await {
                agentgod_files::error::log(&format!("SSH 目标 {} 启动失败: {err}", target.name));
            }
        }
    });

    let app = router(front.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("无法监听本地回环端口")?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            agentgod_files::error::log(&format!("桌面伴生服务异常退出: {err}"));
        }
    });

    Ok(Runtime {
        url: format!("http://{addr}/"),
        local,
        _front: front,
    })
}

async fn ensure_ssh(front: &Arc<FrontState>, target: &SshTarget) -> anyhow::Result<String> {
    let mut guard = front.ssh.lock().await;
    if let Some(service) = guard.get(&target.id) {
        return Ok(service.base_url.clone());
    }
    ssh::service::clean_orphan_uploads(&front.data_dir.join("ssh-tmp").join(&target.id));
    let service = ssh::service::start(
        target.clone(),
        &front.data_dir,
        front.secrets.clone(),
        front.known_hosts.clone(),
    )
    .await?;
    let url = service.base_url.clone();
    guard.insert(target.id.clone(), service);
    Ok(url)
}

fn local_config(data_dir: &Path) -> anyhow::Result<Config> {
    // Tests pin the visible local tree; the shipped app opens the home folder.
    let home = std::env::var_os("AGENTGOD_LOCAL_ROOT")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"));
    let data_root = std::fs::canonicalize(&home).unwrap_or(home);
    let tmp_root = data_dir.join("tmp-local");
    let state_dir = data_dir.join("local-state");
    std::fs::create_dir_all(&tmp_root)?;
    std::fs::create_dir_all(&state_dir)?;
    let tmp_root = std::fs::canonicalize(&tmp_root)?;
    let state_dir = std::fs::canonicalize(&state_dir)?;

    Ok(Config {
        bind: IpAddr::from([127, 0, 0, 1]),
        port: 0,
        data_root,
        tmp_root,
        state_dir: state_dir.clone(),
        tls_dir: state_dir.join("tls"),
        admin_user: "local".to_string(),
        admin_password: None,
        admin_password_hash: None,
        session_ttl: Duration::from_secs(24 * 3600),
        session_max_ttl: Duration::from_secs(3650 * 24 * 3600),
        upload_rate_bps: 0.0,
        download_rate_bps: 0.0,
        max_upload_concurrency: 4,
        max_download_concurrency: 4,
        max_preview_concurrency: 8,
        max_active_uploads: 128,
        max_file_bytes: 1024 * 1024 * 1024 * 1024,
        disk_reserve_bytes: 0,
        rsa_bits: 2048,
        tls_extra_ips: Vec::new(),
        tls_extra_dns: Vec::new(),
        login_max_failures: 8,
        login_window: Duration::from_secs(600),
        local_mode: true,
        tls_enabled: false,
    })
}

struct Route {
    base_url: String,
    allow_invalid_certs: bool,
}

async fn route_for(front: &FrontState, id: &str) -> Option<Route> {
    if id == "local" {
        return front.local_url.as_ref().map(|url| Route {
            base_url: url.clone(),
            allow_invalid_certs: false,
        });
    }
    let target = front
        .config
        .read()
        .expect("config lock")
        .find(id)
        .cloned()?;
    match target {
        TargetEntry::Http(http) => Some(Route {
            base_url: http.url,
            allow_invalid_certs: http.allow_invalid_certs,
        }),
        TargetEntry::Ssh(_) => front.ssh.lock().await.get(id).map(|service| Route {
            base_url: service.base_url.clone(),
            allow_invalid_certs: false,
        }),
    }
}

async fn resolve_active(front: &FrontState) -> Option<(String, Route)> {
    let config = front.config.read().expect("config lock").clone();
    if let Some(active) = config.active.as_deref() {
        if let Some(route) = route_for(front, active).await {
            return Some((active.to_string(), route));
        }
    }
    if front.local_url.is_some() {
        if let Some(route) = route_for(front, "local").await {
            return Some(("local".to_string(), route));
        }
    }
    let first = config.targets.first()?.id().to_string();
    route_for(front, &first).await.map(|route| (first, route))
}

fn router(front: Arc<FrontState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(agentgod_files::web::app_js))
        .route("/style.css", get(agentgod_files::web::style_css))
        .route("/favicon.svg", get(agentgod_files::web::favicon))
        .route("/vendor/mammoth.js", get(agentgod_files::web::mammoth_js))
        .route("/vendor/xlsx.js", get(agentgod_files::web::xlsx_js))
        .route("/app/info", get(app_info))
        .route("/app/servers", post(servers_action))
        .route("/app/local/root", post(local_set_root))
        .route("/app/update/check", get(update_check))
        .route("/app/update/status", get(update_status))
        .route("/app/update/open", post(update_open))
        .route("/app/update/install", post(update_install))
        .route("/app/update/restart", post(update_restart))
        .fallback(proxy)
        .layer(middleware::from_fn(front_guard))
        .with_state(front)
}

/// Loopback hardening for the desktop companion: reject foreign Host headers
/// (DNS rebinding) and cross-origin calls to privileged endpoints.
async fn front_guard(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !agentgod_files::web::is_loopback_host(host) {
        return (StatusCode::MISDIRECTED_REQUEST, "invalid host").into_response();
    }

    let path = request.uri().path();
    if path.starts_with("/api/") || path.starts_with("/app/") {
        let cross_site = request
            .headers()
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.eq_ignore_ascii_case("cross-site"))
            .unwrap_or(false);
        if cross_site {
            return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
        }
        if let Some(origin) = request.headers().get(header::ORIGIN) {
            let allowed = origin
                .to_str()
                .ok()
                .map(is_loopback_origin)
                .unwrap_or(false);
            if !allowed {
                return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
            }
        }
    }
    next.run(request).await
}

fn is_loopback_origin(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && matches!(
            url.host_str(),
            Some("127.0.0.1") | Some("localhost") | Some("::1") | Some("[::1]")
        )
}

async fn index() -> Response {
    let mut response = agentgod_files::web::index().await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(agentgod_files::web::CONTENT_SECURITY_POLICY),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

async fn app_info(State(front): State<Arc<FrontState>>, headers: HeaderMap) -> Response {
    // A tab asking for its own info (SSH roots, local roots) must reach that
    // target's service; only the header-less bootstrap call builds the list.
    if let Some(id) = headers
        .get("x-agentgod-target")
        .and_then(|value| value.to_str().ok())
        .filter(|id| !id.is_empty())
    {
        return forward_target(&front, id, Method::GET, "/app/info", None).await;
    }
    let config = front.config.read().expect("config lock").clone();
    let mut targets = Vec::new();
    if front.local_url.is_some() {
        targets.push(json!({
            "id": "local",
            "name": "本机",
            "kind": "local"
        }));
    }
    for target in &config.targets {
        match target {
            TargetEntry::Http(http) => targets.push(json!({
                "id": http.id,
                "name": http.name,
                "kind": "http",
                "url": http.url,
                "allow_invalid_certs": http.allow_invalid_certs,
            })),
            TargetEntry::Ssh(ssh) => {
                let mut roots = vec![json!({ "name": "根目录", "path": "/" })];
                if let Some(root) = ssh.root.as_deref().filter(|root| *root != "/") {
                    roots.push(json!({ "name": "配置的目录", "path": root }));
                }
                targets.push(json!({
                    "id": ssh.id,
                    "name": ssh.name,
                    "kind": "ssh",
                    "host": ssh.host,
                    "port": ssh.port,
                    "user": ssh.username,
                    "auth": match ssh.auth {
                        SshAuth::Password => "password",
                        SshAuth::Key => "key",
                    },
                    "key_path": ssh.key_path,
                    "root": ssh.root,
                    "roots": roots,
                }));
            }
        }
    }
    let active = resolve_active(&front).await.map(|(id, _)| id);
    let local = match front.local_url.as_ref() {
        Some(url) => fetch_local_info(url).await,
        None => Value::Null,
    };

    Json(json!({
        "mode": "desktop",
        "version": agentgod_files::VERSION,
        "platform": std::env::consts::OS,
        "active": active,
        "targets": targets,
        "local": local,
    }))
    .into_response()
}

async fn fetch_local_info(local_url: &str) -> Value {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return Value::Null,
    };
    match client.get(format!("{local_url}/app/info")).send().await {
        Ok(response) => response.json::<Value>().await.unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

#[derive(Deserialize)]
struct UpdateQuery {
    #[serde(default)]
    force: Option<String>,
}

impl UpdateQuery {
    fn forced(&self) -> bool {
        matches!(
            self.force.as_deref(),
            Some("1") | Some("true") | Some("yes")
        )
    }
}

fn update_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

async fn update_check(
    State(front): State<Arc<FrontState>>,
    axum::extract::Query(query): axum::extract::Query<UpdateQuery>,
) -> Response {
    Json(front.update.check(query.forced()).await).into_response()
}

async fn update_status(State(front): State<Arc<FrontState>>) -> Response {
    Json(front.update.install_status()).into_response()
}

async fn update_open(State(front): State<Arc<FrontState>>) -> Response {
    match front.update.open_download() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(message) => update_error(StatusCode::BAD_GATEWAY, message),
    }
}

async fn update_install(State(front): State<Arc<FrontState>>) -> Response {
    let check = front.update.check(false).await;
    if !check.has_update || !check.silent {
        return update_error(
            StatusCode::CONFLICT,
            "当前没有可用的静默更新包，请下载适配本机的安装包升级",
        );
    }
    match front.update.start_install() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(message) => update_error(StatusCode::BAD_GATEWAY, message),
    }
}

async fn update_restart(State(front): State<Arc<FrontState>>) -> Response {
    match front.update.restart() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(message) => update_error(StatusCode::BAD_GATEWAY, message),
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ServerAction {
    /// HTTP target; the old name is kept so existing UI builds keep working.
    Add {
        name: String,
        url: String,
        #[serde(default)]
        allow_invalid_certs: bool,
    },
    Update {
        id: String,
        name: String,
        url: String,
        #[serde(default)]
        allow_invalid_certs: bool,
    },
    AddSsh {
        name: String,
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        username: String,
        #[serde(default)]
        auth: Option<SshAuth>,
        #[serde(default)]
        key_path: Option<String>,
        #[serde(default)]
        root: Option<String>,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        passphrase: Option<String>,
    },
    UpdateSsh {
        id: String,
        name: String,
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        username: String,
        #[serde(default)]
        auth: Option<SshAuth>,
        #[serde(default)]
        key_path: Option<String>,
        #[serde(default)]
        root: Option<String>,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        passphrase: Option<String>,
        #[serde(default)]
        clear_secrets: bool,
    },
    Remove {
        id: String,
    },
    TrustHost {
        id: String,
        fingerprint: String,
    },
    ForgetHost {
        id: String,
    },
    Test {
        id: String,
    },
}

fn default_ssh_port() -> u16 {
    22
}

struct ActionFailure {
    status: StatusCode,
    payload: Value,
}

impl ActionFailure {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            payload: json!({ "error": message.into() }),
        }
    }

    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    fn gateway(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message)
    }

    /// Keeps the structured error from an SSH service (`code`, `host_key`, …)
    /// so the UI can drive the fingerprint dialog, and adds the `error` alias
    /// the generic error banner reads.
    fn from_service(status: StatusCode, mut body: Value) -> Self {
        if body.is_null() {
            return Self::gateway(format!("目标服务返回错误（{status}）"));
        }
        if body.get("error").is_none() {
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let (Some(message), Value::Object(map)) = (message, &mut body) {
                map.insert("error".to_string(), Value::String(message));
            }
        }
        Self {
            status,
            payload: body,
        }
    }
}

fn clean_name(name: &str) -> Result<String, ActionFailure> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ActionFailure::bad("名称不能为空"));
    }
    if name.chars().count() > 60 {
        return Err(ActionFailure::bad("名称过长"));
    }
    Ok(name.to_string())
}

fn clean_host(host: &str) -> Result<String, ActionFailure> {
    let host = host.trim();
    if host.is_empty() {
        return Err(ActionFailure::bad("主机地址不能为空"));
    }
    if host.contains(char::is_whitespace) || host.contains(['/', '@']) {
        return Err(ActionFailure::bad("主机地址不合法"));
    }
    Ok(host.to_string())
}

fn clean_username(username: &str) -> Result<String, ActionFailure> {
    let username = username.trim();
    if username.is_empty() {
        return Err(ActionFailure::bad("用户名不能为空"));
    }
    if username.contains(char::is_whitespace) || username.contains([':', '/', '@']) {
        return Err(ActionFailure::bad("用户名不合法"));
    }
    Ok(username.to_string())
}

fn clean_remote_root(root: Option<String>) -> Result<Option<String>, ActionFailure> {
    match root
        .as_deref()
        .map(str::trim)
        .filter(|root| !root.is_empty())
    {
        None => Ok(None),
        Some("/") => Ok(None),
        Some(path) => {
            if !path.starts_with('/') {
                return Err(ActionFailure::bad("起始目录必须是绝对路径"));
            }
            Ok(Some(path.trim_end_matches('/').to_string()))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_ssh_target(
    id: String,
    name: String,
    host: String,
    port: u16,
    username: String,
    auth: Option<SshAuth>,
    key_path: Option<String>,
    root: Option<String>,
) -> Result<SshTarget, ActionFailure> {
    if port == 0 {
        return Err(ActionFailure::bad("端口不合法"));
    }
    let key_path = key_path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty());
    let auth = auth.unwrap_or(if key_path.is_some() {
        SshAuth::Key
    } else {
        SshAuth::Password
    });
    if auth == SshAuth::Key && key_path.is_none() {
        return Err(ActionFailure::bad("请选择私钥文件"));
    }
    Ok(SshTarget {
        id,
        name: clean_name(&name)?,
        host: clean_host(&host)?,
        port,
        username: clean_username(&username)?,
        auth,
        key_path,
        root: clean_remote_root(root)?,
    })
}

fn ssh_target_json(target: &SshTarget) -> Value {
    json!({
        "ok": true,
        "id": target.id,
        "kind": "ssh",
        "name": target.name,
        "host": target.host,
        "port": target.port,
        "user": target.username,
        "auth": target.auth,
        "key_path": target.key_path,
        "root": target.root,
    })
}

async fn ensure_route(front: &Arc<FrontState>, id: &str) -> Result<Route, ActionFailure> {
    if let Some(route) = route_for(front, id).await {
        return Ok(route);
    }
    if id == "local" {
        return Err(ActionFailure::not_found("本机文件服务不可用"));
    }
    let target = {
        let config = front.config.read().expect("config lock");
        config.find(id).cloned()
    };
    match target {
        Some(TargetEntry::Ssh(target)) => {
            let url = ensure_ssh(front, &target)
                .await
                .map_err(|err| ActionFailure::gateway(format!("SSH 服务启动失败: {err}")))?;
            Ok(Route {
                base_url: url,
                allow_invalid_certs: false,
            })
        }
        Some(TargetEntry::Http(_)) => Err(ActionFailure::not_found("目标地址尚未就绪")),
        None => Err(ActionFailure::not_found("连接目标不存在")),
    }
}

async fn servers_action(
    State(front): State<Arc<FrontState>>,
    Json(action): Json<ServerAction>,
) -> Response {
    match handle_action(&front, action).await {
        Ok(value) => Json(value).into_response(),
        Err(failure) => (failure.status, Json(failure.payload)).into_response(),
    }
}

async fn handle_action(
    front: &Arc<FrontState>,
    action: ServerAction,
) -> Result<Value, ActionFailure> {
    match action {
        ServerAction::Add {
            name,
            url,
            allow_invalid_certs,
        } => {
            let name = clean_name(&name)?;
            let url = config::normalize_http_url(&url)
                .ok_or_else(|| ActionFailure::bad("服务器地址无效"))?;
            let id = config::new_id();
            {
                let mut config = front.config.write().expect("config lock");
                config.targets.push(TargetEntry::Http(HttpTarget {
                    id: id.clone(),
                    name,
                    url,
                    allow_invalid_certs,
                }));
                config.active = Some(id.clone());
            }
            front.save();
            Ok(json!({ "ok": true, "id": id }))
        }
        ServerAction::Update {
            id,
            name,
            url,
            allow_invalid_certs,
        } => {
            let name = clean_name(&name)?;
            let url = config::normalize_http_url(&url)
                .ok_or_else(|| ActionFailure::bad("服务器地址无效"))?;
            {
                let mut config = front.config.write().expect("config lock");
                match config.find_mut(&id) {
                    Some(TargetEntry::Http(target)) => {
                        target.name = name;
                        target.url = url;
                        target.allow_invalid_certs = allow_invalid_certs;
                    }
                    Some(TargetEntry::Ssh(_)) => {
                        return Err(ActionFailure::bad("该目标不是服务器地址"));
                    }
                    None => return Err(ActionFailure::not_found("目标不存在")),
                }
            }
            front.save();
            Ok(json!({ "ok": true, "id": id }))
        }
        ServerAction::AddSsh {
            name,
            host,
            port,
            username,
            auth,
            key_path,
            root,
            password,
            passphrase,
        } => {
            let target = build_ssh_target(
                config::new_id(),
                name,
                host,
                port,
                username,
                auth,
                key_path,
                root,
            )?;
            if target.auth == SshAuth::Password
                && password.as_deref().map(str::is_empty).unwrap_or(true)
            {
                return Err(ActionFailure::bad("请输入登录密码"));
            }
            store_secret(front, &target.id, "password", &password)?;
            store_secret(front, &target.id, "passphrase", &passphrase)?;
            {
                let mut config = front.config.write().expect("config lock");
                config.targets.push(TargetEntry::Ssh(target.clone()));
                config.active = Some(target.id.clone());
            }
            front.save();
            ensure_ssh(front, &target)
                .await
                .map_err(|err| ActionFailure::gateway(format!("SSH 服务启动失败: {err}")))?;
            Ok(ssh_target_json(&target))
        }
        ServerAction::UpdateSsh {
            id,
            name,
            host,
            port,
            username,
            auth,
            key_path,
            root,
            password,
            passphrase,
            clear_secrets,
        } => {
            let existing = {
                let config = front.config.read().expect("config lock");
                config.find(&id).cloned()
            };
            let Some(TargetEntry::Ssh(old)) = existing else {
                return Err(ActionFailure::not_found("SSH 目标不存在"));
            };
            let target = build_ssh_target(
                old.id.clone(),
                name,
                host,
                port,
                username,
                auth.or(Some(old.auth)),
                key_path.or_else(|| old.key_path.clone()),
                root.or_else(|| old.root.clone().or(Some(String::new()))),
            )?;
            if target.auth == SshAuth::Password {
                let supplied = password
                    .as_deref()
                    .map(|value| !value.is_empty())
                    .unwrap_or(false);
                let stored = front
                    .secrets
                    .get(&secrets::account(&id, "password"))
                    .ok()
                    .flatten()
                    .is_some();
                if !supplied && (!stored || clear_secrets) {
                    return Err(ActionFailure::bad("请输入登录密码"));
                }
            }
            if clear_secrets {
                clear_secret(front, &id, "password");
                clear_secret(front, &id, "passphrase");
            } else {
                store_secret(front, &id, "password", &password)?;
                store_secret(front, &id, "passphrase", &passphrase)?;
            }
            {
                let mut config = front.config.write().expect("config lock");
                match config.find_mut(&id) {
                    Some(slot) => *slot = TargetEntry::Ssh(target.clone()),
                    None => return Err(ActionFailure::not_found("SSH 目标不存在")),
                }
            }
            front.save();
            // Rebuild the service so the new host/credentials take effect and
            // any cached connection is dropped.
            front.ssh.lock().await.remove(&id);
            ensure_ssh(front, &target)
                .await
                .map_err(|err| ActionFailure::gateway(format!("SSH 服务启动失败: {err}")))?;
            Ok(ssh_target_json(&target))
        }
        ServerAction::Remove { id } => {
            if id == "local" {
                return Err(ActionFailure::bad("不能删除本机"));
            }
            let removed = {
                let mut config = front.config.write().expect("config lock");
                let before = config.targets.len();
                config.targets.retain(|target| target.id() != id);
                let removed = config.targets.len() != before;
                if removed {
                    config.normalize_active(front.local_url.is_some());
                }
                removed
            };
            if !removed {
                return Err(ActionFailure::not_found("目标不存在"));
            }
            front.save();
            front.ssh.lock().await.remove(&id);
            clear_secret(front, &id, "password");
            clear_secret(front, &id, "passphrase");
            Ok(json!({ "ok": true }))
        }
        ServerAction::TrustHost { id, fingerprint } => {
            let route = ensure_route(front, &id).await?;
            post_service(
                &route,
                "/app/ssh/trust",
                json!({ "fingerprint": fingerprint }),
            )
            .await
        }
        ServerAction::ForgetHost { id } => {
            let route = ensure_route(front, &id).await?;
            post_service(&route, "/app/ssh/forget", json!({})).await
        }
        ServerAction::Test { id } => {
            let target = {
                let config = front.config.read().expect("config lock");
                config.find(&id).cloned()
            };
            match target {
                Some(TargetEntry::Ssh(_)) => {
                    let route = ensure_route(front, &id).await?;
                    post_service(&route, "/app/ssh/test", json!({})).await
                }
                Some(TargetEntry::Http(http)) => {
                    let client = front.client(&id, http.allow_invalid_certs);
                    let response = client
                        .get(format!("{}/app/info", http.url))
                        .timeout(Duration::from_secs(10))
                        .send()
                        .await
                        .map_err(|err| ActionFailure::gateway(format!("无法连接服务器: {err}")))?;
                    let status = response.status();
                    let body = response.json::<Value>().await.unwrap_or(Value::Null);
                    if status.is_success() {
                        Ok(json!({ "ok": true, "info": body }))
                    } else {
                        Err(ActionFailure::from_service(status, body))
                    }
                }
                None => Err(ActionFailure::not_found("目标不存在")),
            }
        }
    }
}

fn store_secret(
    front: &FrontState,
    id: &str,
    field: &str,
    value: &Option<String>,
) -> Result<(), ActionFailure> {
    let Some(value) = value.as_deref().filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    front
        .secrets
        .set(&secrets::account(id, field), value)
        .map_err(|err| ActionFailure::internal(err.to_string()))
}

fn clear_secret(front: &FrontState, id: &str, field: &str) {
    let _ = front.secrets.delete(&secrets::account(id, field));
}

async fn post_service(route: &Route, path: &str, payload: Value) -> Result<Value, ActionFailure> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .danger_accept_invalid_certs(route.allow_invalid_certs)
        .build()
        .map_err(|err| ActionFailure::internal(err.to_string()))?;
    let response = client
        .post(format!("{}{}", route.base_url, path))
        .json(&payload)
        .send()
        .await
        .map_err(|err| ActionFailure::gateway(format!("无法连接目标服务: {err}")))?;
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    if status.is_success() {
        Ok(body)
    } else {
        Err(ActionFailure::from_service(status, body))
    }
}

/// Sends one request to a target's loopback service, starting an SSH service
/// on demand. Used for `/app/info` and root switching, which the companion
/// itself also serves and therefore cannot route through `proxy`.
async fn forward_target(
    front: &Arc<FrontState>,
    id: &str,
    method: Method,
    path: &str,
    body: Option<Vec<u8>>,
) -> Response {
    let route = match ensure_route(front, id).await {
        Ok(route) => route,
        Err(failure) => return (failure.status, Json(failure.payload)).into_response(),
    };
    let client = front.client(&format!("forward:{id}"), route.allow_invalid_certs);
    let url = format!("{}{}", route.base_url.trim_end_matches('/'), path);
    let mut builder = client.request(to_reqwest_method(&method), url);
    if let Some(body) = body {
        builder = builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    let response = match builder.timeout(Duration::from_secs(20)).send().await {
        Ok(response) => response,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("目标服务不可用: {err}") })),
            )
                .into_response()
        }
    };
    let status = response.status();
    let headers = response.headers().clone();
    let stream = response.bytes_stream();
    let mut out = Response::new(Body::from_stream(stream));
    *out.status_mut() = status;
    for name in FORWARD_RESPONSE_HEADERS {
        if let Some(value) = headers.get(*name) {
            if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) {
                out.headers_mut().insert(header_name, value.clone());
            }
        }
    }
    out
}

async fn local_set_root(State(front): State<Arc<FrontState>>, request: Request) -> Response {
    let target = request
        .headers()
        .get("x-agentgod-target")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let id = match target {
        Some(id) => id,
        None => match resolve_active(&front).await {
            Some((id, _)) => id,
            None => "local".to_string(),
        },
    };
    let body = match axum::body::to_bytes(request.into_body(), 1024 * 1024).await {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid body").into_response(),
    };
    forward_target(
        &front,
        &id,
        Method::POST,
        "/app/local/root",
        Some(body.to_vec()),
    )
    .await
}

async fn proxy(State(front): State<Arc<FrontState>>, request: Request) -> Response {
    let requested = request
        .headers()
        .get("x-agentgod-target")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (target_id, route) = match requested {
        Some(id) => match route_for(&front, &id).await {
            Some(route) => (id, route),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "连接目标不存在或尚未就绪，请刷新服务器列表" })),
                )
                    .into_response()
            }
        },
        None => match resolve_active(&front).await {
            Some(pair) => pair,
            None => {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({ "error": "尚未配置服务器，请先在连接设置中添加" })),
                )
                    .into_response()
            }
        },
    };

    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let url = format!("{}{}", route.base_url.trim_end_matches('/'), path_and_query);
    let method = request.method().clone();
    let headers = request.headers().clone();

    let body = match axum::body::to_bytes(request.into_body(), 32 * 1024 * 1024).await {
        Ok(body) => body,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({ "error": "请求体过大" })),
            )
                .into_response()
        }
    };

    let client = front.client(&target_id, route.allow_invalid_certs);
    let mut builder = client.request(to_reqwest_method(&method), &url);
    for name in FORWARD_REQUEST_HEADERS {
        if let Some(value) = headers.get(*name) {
            builder = builder.header(*name, value);
        }
    }

    let response = match builder.body(body.to_vec()).send().await {
        Ok(response) => response,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("无法连接服务器: {err}") })),
            )
                .into_response()
        }
    };

    let status = response.status();
    let response_headers = response.headers().clone();
    let stream = response.bytes_stream();
    let mut out = Response::new(Body::from_stream(stream));
    *out.status_mut() = status;
    for name in FORWARD_RESPONSE_HEADERS {
        if let Some(value) = response_headers.get(*name) {
            if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) {
                out.headers_mut().insert(header_name, value.clone());
            }
        }
    }
    out
}

fn to_reqwest_method(method: &Method) -> reqwest::Method {
    reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_origins_are_accepted() {
        for origin in [
            "http://127.0.0.1:1234",
            "http://localhost:9000",
            "https://[::1]:8443",
        ] {
            assert!(is_loopback_origin(origin), "{origin}");
        }
        for origin in ["https://evil.example", "file:///tmp/x", "null"] {
            assert!(!is_loopback_origin(origin), "{origin}");
        }
    }

    #[test]
    fn config_file_is_shared() {
        assert_eq!(config::CONFIG_FILENAME, "targets.json");
    }

    #[test]
    fn ssh_targets_store_no_secrets() {
        let target = SshTarget {
            id: "t1".to_string(),
            name: "VPS".to_string(),
            host: "203.0.113.7".to_string(),
            port: 22,
            username: "root".to_string(),
            auth: SshAuth::Password,
            key_path: None,
            root: Some("/var/www".to_string()),
        };
        let raw = serde_json::to_string(&TargetEntry::Ssh(target)).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert!(value.get("password").is_none());
        assert!(value.get("passphrase").is_none());
        assert!(raw.contains("\"kind\":\"ssh\""));
    }
}
