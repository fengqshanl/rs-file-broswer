use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderName, HeaderValue},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Router,
};
use tokio::time::MissedTickBehavior;

use crate::{app, auth, error::log, files, preview, state::AppState};

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; frame-src 'self' blob:; connect-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none';";

pub fn router(state: Arc<AppState>) -> Router {
    let local_mode = state.cfg.local_mode;
    let router = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/favicon.svg", get(favicon))
        .route("/healthz", get(health))
        .route("/app/info", get(app::info))
        .route("/app/local/root", post(app::set_root))
        .route("/api/local-login", get(auth::local_login))
        .route("/api/pubkey", get(auth::pubkey))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/account/update", post(auth::update_credentials))
        .route("/api/list", post(files::list))
        .route("/api/mkdir", post(files::mkdir))
        .route("/api/ensure_dirs", post(files::ensure_dirs))
        .route("/api/rename", post(files::rename))
        .route("/api/delete", post(files::delete))
        .route("/api/download", post(files::download))
        .route("/api/download_folder", post(files::download_folder))
        .route("/api/preview/token", post(preview::create_token))
        .route("/preview/{token}", get(preview::serve))
        .route("/vendor/mammoth.js", get(mammoth_js))
        .route("/vendor/xlsx.js", get(xlsx_js))
        .route("/api/upload/init", post(files::upload_init))
        .route("/api/upload/chunk/{id}/{seq}", put(files::upload_chunk))
        .route("/api/upload/finish", post(files::upload_finish))
        .route("/api/upload/abort", post(files::upload_abort))
        .with_state(state);
    // The desktop companion is unauthenticated and exposes the local disk;
    // refuse requests whose Host is not a loopback name to blunt DNS rebinding.
    let router = if local_mode {
        router.layer(middleware::from_fn(loopback_host_guard))
    } else {
        router
    };
    router
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(log_requests))
}

pub fn is_loopback_host(value: &str) -> bool {
    let host = if let Some(rest) = value.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        value.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

async fn loopback_host_guard(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !is_loopback_host(host) {
        return (axum::http::StatusCode::MISDIRECTED_REQUEST, "invalid host").into_response();
    }
    next.run(request).await
}

pub fn spawn_cleanup(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(600));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now = Instant::now();
            state
                .sessions
                .retain(|_, session| now < session.expires_at && now < session.hard_expires_at);
            state.login_guard.cleanup();
            files::cleanup_stale_uploads(&state).await;
            preview::cleanup_expired(&state);
        }
    });
}

async fn log_requests(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    if path.starts_with("/api/") && !path.starts_with("/api/upload/chunk/") {
        log(&format!(
            "{method} {path} -> {} ({} ms)",
            response.status().as_u16(),
            started.elapsed().as_millis()
        ));
    }
    response
}

async fn security_headers(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let is_api = path.starts_with("/api/");
    // Preview responses carry their own framing/CSP policy so that PDFs and
    // sandboxed HTML can render inside the app.
    let is_preview = path.starts_with("/preview/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if is_preview {
        headers
            .entry(header::X_CONTENT_TYPE_OPTIONS)
            .or_insert(HeaderValue::from_static("nosniff"));
        return response;
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(CSP),
    );
    if is_api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn asset(content: &'static str, content_type: &'static str, cache: &'static str) -> Response {
    let mut response = Response::new(Body::from(content));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

pub async fn index() -> Response {
    asset(
        include_str!("../static/index.html"),
        "text/html; charset=utf-8",
        "no-store",
    )
}

pub async fn app_js() -> Response {
    asset(
        include_str!("../static/app.js"),
        "application/javascript; charset=utf-8",
        "no-cache",
    )
}

pub async fn style_css() -> Response {
    asset(
        include_str!("../static/style.css"),
        "text/css; charset=utf-8",
        "no-cache",
    )
}

pub async fn favicon() -> Response {
    asset(
        include_str!("../static/favicon.svg"),
        "image/svg+xml",
        "public, max-age=86400",
    )
}

pub async fn mammoth_js() -> Response {
    asset(
        include_str!("../static/vendor/mammoth.browser.min.js"),
        "application/javascript; charset=utf-8",
        "public, max-age=604800",
    )
}

pub async fn xlsx_js() -> Response {
    asset(
        include_str!("../static/vendor/xlsx.full.min.js"),
        "application/javascript; charset=utf-8",
        "public, max-age=604800",
    )
}

async fn health() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "ok")
}
