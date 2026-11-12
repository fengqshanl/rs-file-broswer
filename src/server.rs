use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use axum_server::Handle;
use tokio::task::JoinHandle;

use crate::{
    config::Config, credentials::Credentials, crypto::Crypto, error::log, files, state::AppState,
    tls, web,
};

/// A running companion HTTP service (desktop local mode or tests).
pub struct HttpServer {
    pub addr: SocketAddr,
    pub state: Arc<AppState>,
    task: JoinHandle<()>,
}

impl HttpServer {
    pub fn abort(&self) {
        self.task.abort();
    }
}

pub fn build_state(cfg: Arc<Config>) -> anyhow::Result<Arc<AppState>> {
    let crypto = Crypto::init(&cfg)?;
    let (credentials, credentials_path) = if cfg.local_mode {
        Credentials::local()
    } else {
        let (credentials, path) = Credentials::load_or_init(&cfg)?;
        log(&format!("管理员账号: {}", credentials.username));
        (credentials, path)
    };
    files::clean_orphan_uploads(&cfg);
    let state = Arc::new(AppState::new(
        cfg.clone(),
        crypto,
        credentials,
        credentials_path,
    ));
    web::spawn_cleanup(state.clone());
    Ok(state)
}

/// Starts a plain-HTTP instance bound to the given address. Used by the
/// desktop local companion, which only ever listens on loopback.
pub async fn start_http(cfg: Arc<Config>) -> anyhow::Result<HttpServer> {
    let state = build_state(cfg.clone())?;
    let app = web::router(state.clone());
    let address = SocketAddr::new(cfg.bind, cfg.port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("无法监听 {address}"))?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        if let Err(err) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        {
            log(&format!("本地 HTTP 服务异常退出: {err}"));
        }
    });
    log(&format!("AgentGod Files 本地服务启动于 http://{addr}"));
    Ok(HttpServer { addr, state, task })
}

pub async fn run_from_env() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cfg = Arc::new(Config::from_env()?);

    if !cfg.tls_enabled {
        let state = build_state(cfg.clone())?;
        let app = web::router(state);
        let address = SocketAddr::new(cfg.bind, cfg.port);
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .with_context(|| format!("无法监听 {address}"))?;
        log(&format!(
            "AgentGod Files 启动于 http://{} | 数据目录: {}",
            listener.local_addr()?,
            cfg.data_root.display()
        ));
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .context("HTTP 服务异常退出")?;
        return Ok(());
    }

    let (cert_path, key_path) = tls::ensure_self_signed(&cfg)?;
    let state = build_state(cfg.clone())?;
    let app = web::router(state);
    let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
        .await
        .with_context(|| {
            format!(
                "加载 TLS 证书失败: {} / {}",
                cert_path.display(),
                key_path.display()
            )
        })?;

    let address = SocketAddr::new(cfg.bind, cfg.port);
    let handle = Handle::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        log("收到退出信号，正在优雅关闭");
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(15)));
    });

    log(&format!(
        "AgentGod Files 启动于 https://{}:{}",
        cfg.bind, cfg.port
    ));
    log(&format!(
        "数据目录: {} | 上传限速: {} | 下载限速: {} | 并发上传/下载: {}/{}",
        cfg.data_root.display(),
        rate_label(cfg.upload_rate_bps),
        rate_label(cfg.download_rate_bps),
        cfg.max_upload_concurrency,
        cfg.max_download_concurrency,
    ));

    axum_server::bind_rustls(address, tls_config)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .context("HTTPS 服务异常退出")?;

    Ok(())
}

fn rate_label(bytes_per_second: f64) -> String {
    if bytes_per_second > 0.0 {
        format!(
            "{} MB/s",
            (bytes_per_second / (1024.0 * 1024.0)).round() as u64
        )
    } else {
        "不限速".to_string()
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
