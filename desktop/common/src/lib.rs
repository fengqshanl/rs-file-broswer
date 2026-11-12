//! Shared runtime for both desktop companions.
//!
//! Each app hosts a tiny loopback HTTP server. It serves the same web UI as the
//! standalone server and routes `/api/*` and `/preview/*` either to an embedded
//! local file service (desktop local app) or to the currently selected remote
//! AgentGod Files server. Because the webview loads `http://127.0.0.1:<port>/`,
//! the existing same-origin frontend works unchanged and streaming is preserved
//! by proxying response bodies with backpressure.

use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use anyhow::Context;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use agentgod_files::{
    config::Config,
    server::{start_http, HttpServer},
};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMode {
    Local,
    Client,
}

impl AppMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AppMode::Local => "local",
            AppMode::Client => "remote",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerEntry {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub allow_invalid_certs: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServersConfig {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
}

pub struct FrontState {
    pub mode: AppMode,
    pub config_path: PathBuf,
    pub config: RwLock<ServersConfig>,
    pub local_url: Option<String>,
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
        let _ = save_config(&self.config_path, &self.config.read().expect("config lock"));
    }
}

pub struct Runtime {
    pub url: String,
    pub local: Option<HttpServer>,
    _front: Arc<FrontState>,
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("AgentGodFiles")
}

pub async fn start(mode: AppMode) -> anyhow::Result<Runtime> {
    let data_dir = data_dir();
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("无法创建数据目录: {}", data_dir.display()))?;

    let local = match mode {
        AppMode::Local => {
            let cfg = local_config(&data_dir)?;
            Some(start_http(Arc::new(cfg)).await?)
        }
        AppMode::Client => None,
    };
    let local_url = local
        .as_ref()
        .map(|server| format!("http://{}", server.addr));

    let config_path = data_dir.join(config_filename(mode));
    let mut config = load_config(&config_path);
    normalize_active(&mut config, local_url.is_some());
    // The local app always opens on this machine's files; saved remote
    // servers stay one dropdown away and are never the startup target.
    if mode == AppMode::Local {
        config.active = Some("local".to_string());
    }
    save_config(&config_path, &config)?;

    let front = Arc::new(FrontState {
        mode,
        config_path,
        config: RwLock::new(config),
        local_url,
        clients: Mutex::new(HashMap::new()),
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

fn local_config(data_dir: &Path) -> anyhow::Result<Config> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
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

fn load_config(path: &Path) -> ServersConfig {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save_config(path: &Path, config: &ServersConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_vec_pretty(config)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn config_filename(mode: AppMode) -> &'static str {
    match mode {
        AppMode::Local => "servers-local.json",
        AppMode::Client => "servers-client.json",
    }
}

fn normalize_active(config: &mut ServersConfig, has_local: bool) {
    let valid = config.active.as_deref().map(|active| {
        (has_local && active == "local") || config.servers.iter().any(|server| server.id == active)
    });
    let needs_fix = !matches!(valid, Some(true));
    if needs_fix {
        config.active = if has_local {
            Some("local".to_string())
        } else {
            config.servers.first().map(|server| server.id.clone())
        };
    }
}

fn pick_active(config: &ServersConfig, local_url: Option<&str>) -> Option<(String, String, bool)> {
    if let Some(active) = config.active.as_deref() {
        if active == "local" {
            if let Some(local_url) = local_url {
                return Some(("local".to_string(), local_url.to_string(), false));
            }
        } else if let Some(server) = config.servers.iter().find(|server| server.id == active) {
            return Some((
                server.id.clone(),
                server.url.clone(),
                server.allow_invalid_certs,
            ));
        }
    }
    if let Some(local_url) = local_url {
        return Some(("local".to_string(), local_url.to_string(), false));
    }
    config.servers.first().map(|server| {
        (
            server.id.clone(),
            server.url.clone(),
            server.allow_invalid_certs,
        )
    })
}

fn resolve_active(front: &FrontState) -> Option<(String, String, bool)> {
    let config = front.config.read().expect("config lock").clone();
    pick_active(&config, front.local_url.as_deref())
}

pub fn router(front: Arc<FrontState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(agentgod_files::web::app_js))
        .route("/style.css", get(agentgod_files::web::style_css))
        .route("/favicon.svg", get(agentgod_files::web::favicon))
        .route("/vendor/mammoth.js", get(agentgod_files::web::mammoth_js))
        .route("/vendor/xlsx.js", get(agentgod_files::web::xlsx_js))
        .route("/app/info", get(app_info))
        .route("/app/active", post(set_active))
        .route("/app/servers", post(servers_action))
        .route("/app/local/root", post(local_set_root))
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

async fn app_info(State(front): State<Arc<FrontState>>) -> Response {
    let config = front.config.read().expect("config lock").clone();
    let mut targets = Vec::new();
    if front.local_url.is_some() {
        targets.push(json!({
            "id": "local",
            "name": "本机",
            "kind": "local"
        }));
    }
    for server in &config.servers {
        targets.push(json!({
            "id": server.id,
            "name": server.name,
            "kind": "remote",
            "url": server.url,
            "allow_invalid_certs": server.allow_invalid_certs,
        }));
    }
    let active = resolve_active(&front).map(|(id, _, _)| id);
    let local = match front.local_url.as_ref() {
        Some(url) => fetch_local_info(url).await,
        None => Value::Null,
    };

    Json(json!({
        "mode": front.mode.as_str(),
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
struct ActiveBody {
    id: String,
}

async fn set_active(
    State(front): State<Arc<FrontState>>,
    Json(body): Json<ActiveBody>,
) -> Response {
    let mut config = front.config.write().expect("config lock");
    let valid = (front.local_url.is_some() && body.id == "local")
        || config.servers.iter().any(|server| server.id == body.id);
    if !valid {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "目标不存在" })),
        )
            .into_response();
    }
    config.active = Some(body.id.clone());
    drop(config);
    front.save();
    Json(json!({ "ok": true, "active": body.id })).into_response()
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ServerAction {
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
    Remove {
        id: String,
    },
}

async fn servers_action(
    State(front): State<Arc<FrontState>>,
    Json(action): Json<ServerAction>,
) -> Response {
    let mut config = front.config.write().expect("config lock");
    let result = match action {
        ServerAction::Add {
            name,
            url,
            allow_invalid_certs,
        } => match normalize_server(&name, &url) {
            Ok(url) => {
                let id = format!("{:016x}", rand::random::<u64>());
                config.servers.push(ServerEntry {
                    id: id.clone(),
                    name: name.trim().to_string(),
                    url,
                    allow_invalid_certs,
                });
                if config.active.is_none() {
                    config.active = Some(id.clone());
                }
                Ok(json!({ "ok": true, "id": id }))
            }
            Err(message) => Err(message),
        },
        ServerAction::Update {
            id,
            name,
            url,
            allow_invalid_certs,
        } => match normalize_server(&name, &url) {
            Ok(url) => {
                if let Some(server) = config.servers.iter_mut().find(|server| server.id == id) {
                    server.name = name.trim().to_string();
                    server.url = url;
                    server.allow_invalid_certs = allow_invalid_certs;
                    Ok(json!({ "ok": true, "id": id }))
                } else {
                    Err("服务器不存在".to_string())
                }
            }
            Err(message) => Err(message),
        },
        ServerAction::Remove { id } => {
            config.servers.retain(|server| server.id != id);
            if config.active.as_deref() == Some(id.as_str()) {
                config.active = if front.local_url.is_some() {
                    Some("local".to_string())
                } else {
                    config.servers.first().map(|server| server.id.clone())
                };
            }
            Ok(json!({ "ok": true }))
        }
    };
    drop(config);
    match result {
        Ok(body) => {
            front.save();
            Json(body).into_response()
        }
        Err(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
    }
}

fn normalize_server(name: &str, url: &str) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("请填写名称".to_string());
    }
    let trimmed = url.trim();
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = reqwest::Url::parse(&candidate).map_err(|_| "URL 格式不正确".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("只支持 http 或 https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("URL 缺少主机名".to_string());
    }
    Ok(candidate.trim_end_matches('/').to_string())
}

async fn local_set_root(State(front): State<Arc<FrontState>>, body: Bytes) -> Response {
    let Some(local_url) = front.local_url.as_ref() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "本机模式不可用" })),
        )
            .into_response();
    };
    let client = front.client("local-admin", false);
    match client
        .post(format!("{local_url}/app/local/root"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            let bytes = response.bytes().await.unwrap_or_default();
            let mut out = Response::new(Body::from(bytes));
            *out.status_mut() = status;
            if let Ok(value) = HeaderValue::from_str(&content_type) {
                out.headers_mut().insert(header::CONTENT_TYPE, value);
            }
            out
        }
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("本机服务不可用: {err}") })),
        )
            .into_response(),
    }
}

async fn proxy(State(front): State<Arc<FrontState>>, request: Request) -> Response {
    let Some((target_id, base_url, allow_invalid_certs)) = resolve_active(&front) else {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "尚未配置服务器，请先在连接设置中添加" })),
        )
            .into_response();
    };

    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let url = format!("{}{}", base_url.trim_end_matches('/'), path_and_query);
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

    let client = front.client(&target_id, allow_invalid_certs);
    let mut builder = client.request(to_reqwest_method(&method), &url);
    for name in FORWARD_REQUEST_HEADERS {
        if let Some(value) = headers.get(*name) {
            builder = builder.header(*name, value);
        }
    }

    let response = match builder.body(body).send().await {
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

    fn server(id: &str, url: &str, allow_invalid_certs: bool) -> ServerEntry {
        ServerEntry {
            id: id.to_string(),
            name: id.to_string(),
            url: url.to_string(),
            allow_invalid_certs,
        }
    }

    #[test]
    fn config_files_are_per_app() {
        assert_eq!(config_filename(AppMode::Local), "servers-local.json");
        assert_eq!(config_filename(AppMode::Client), "servers-client.json");
    }

    #[test]
    fn server_urls_are_normalized() {
        assert_eq!(
            normalize_server("NAS", "192.168.1.10:8090").unwrap(),
            "https://192.168.1.10:8090"
        );
        assert_eq!(
            normalize_server("NAS", " http://127.0.0.1:8090/ ").unwrap(),
            "http://127.0.0.1:8090"
        );
        assert!(normalize_server("  ", "https://example.com").is_err());
        assert!(normalize_server("NAS", "ftp://example.com").is_err());
        assert!(normalize_server("NAS", "https://").is_err());
    }

    #[test]
    fn active_target_is_repaired_when_stale() {
        let mut config = ServersConfig {
            active: Some("missing".to_string()),
            servers: vec![server("a", "https://a.example", false)],
        };
        normalize_active(&mut config, true);
        assert_eq!(config.active.as_deref(), Some("local"));

        normalize_active(&mut config, false);
        assert_eq!(config.active.as_deref(), Some("a"));

        let mut empty = ServersConfig::default();
        normalize_active(&mut empty, false);
        assert!(empty.active.is_none());
    }

    #[test]
    fn explicit_active_server_wins_without_local_mode() {
        let config = ServersConfig {
            active: Some("b".to_string()),
            servers: vec![
                server("a", "https://a.example", false),
                server("b", "https://b.example", true),
            ],
        };
        assert_eq!(
            pick_active(&config, None),
            Some(("b".to_string(), "https://b.example".to_string(), true))
        );
    }

    #[test]
    fn local_target_is_used_when_selected_or_when_no_remote_is_active() {
        let config = ServersConfig {
            active: Some("local".to_string()),
            servers: vec![server("a", "https://a.example", false)],
        };
        assert_eq!(
            pick_active(&config, Some("http://127.0.0.1:1234")),
            Some((
                "local".to_string(),
                "http://127.0.0.1:1234".to_string(),
                false
            ))
        );

        let fallback = ServersConfig {
            active: None,
            servers: vec![server("a", "https://a.example", false)],
        };
        assert_eq!(
            pick_active(&fallback, None),
            Some(("a".to_string(), "https://a.example".to_string(), false))
        );
        assert_eq!(pick_active(&ServersConfig::default(), None), None);
    }

    #[test]
    fn loopback_origins_are_accepted() {
        for origin in [
            "http://127.0.0.1:8090",
            "http://localhost:8090",
            "https://[::1]:8090",
        ] {
            assert!(is_loopback_origin(origin), "{origin}");
        }
        for origin in ["https://evil.example", "null", "file:///tmp/x"] {
            assert!(!is_loopback_origin(origin), "{origin}");
        }
    }
}
