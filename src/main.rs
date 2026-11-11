mod api;
mod archive;
mod auth;
mod config;
mod credentials;
mod crypto;
mod error;
mod files;
mod preview;
mod rate;
mod state;
mod tls;
mod web;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use axum_server::Handle;

use crate::{config::Config, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cfg = Arc::new(Config::from_env()?);
    let crypto = crypto::Crypto::init(&cfg)?;
    let (credentials, credentials_path) = credentials::Credentials::load_or_init(&cfg)?;
    let admin_username = credentials.username.clone();
    let (cert_path, key_path) = tls::ensure_self_signed(&cfg)?;

    files::clean_orphan_uploads(&cfg);
    let state = Arc::new(AppState::new(
        cfg.clone(),
        crypto,
        credentials,
        credentials_path,
    ));
    web::spawn_cleanup(state.clone());

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
        error::log("收到退出信号，正在优雅关闭");
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(15)));
    });

    error::log(&format!(
        "AgentGod Files 启动于 https://{}:{}",
        cfg.bind, cfg.port
    ));
    let upload_rate = if cfg.upload_rate_bps > 0.0 {
        format!(
            "{} MB/s",
            (cfg.upload_rate_bps / (1024.0 * 1024.0)).round() as u64
        )
    } else {
        "不限速".to_string()
    };
    let download_rate = if cfg.download_rate_bps > 0.0 {
        format!(
            "{} MB/s",
            (cfg.download_rate_bps / (1024.0 * 1024.0)).round() as u64
        )
    } else {
        "不限速".to_string()
    };
    error::log(&format!(
        "数据目录: {} | 上传限速: {} | 下载限速: {} | 并发上传/下载: {}/{}",
        cfg.data_root.display(),
        upload_rate,
        download_rate,
        cfg.max_upload_concurrency,
        cfg.max_download_concurrency,
    ));
    error::log(&format!("管理员账号: {admin_username}"));

    axum_server::bind_rustls(address, tls_config)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .context("HTTPS 服务异常退出")?;

    Ok(())
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
