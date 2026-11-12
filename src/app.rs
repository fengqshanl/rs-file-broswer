use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    error::{log, ApiError},
    state::AppState,
};

#[derive(Clone, Serialize)]
pub struct RootEntry {
    pub name: String,
    pub path: String,
}

/// Roots offered by the desktop "local" companion: the user's home, the
/// filesystem root (or every drive on Windows), and the current root if it is
/// not one of those.
pub fn local_roots(current: &Path) -> Vec<RootEntry> {
    let mut roots = Vec::new();

    if let Some(home) = dirs::home_dir() {
        roots.push(RootEntry {
            name: "主目录".to_string(),
            path: home.display().to_string(),
        });
    }

    #[cfg(windows)]
    {
        for letter in b'C'..=b'Z' {
            let path = format!("{}:\\", letter as char);
            if Path::new(&path).exists() {
                roots.push(RootEntry {
                    name: format!("{}: 盘", letter as char),
                    path,
                });
            }
        }
    }
    #[cfg(not(windows))]
    {
        roots.push(RootEntry {
            name: "根目录".to_string(),
            path: "/".to_string(),
        });
    }

    if !roots.iter().any(|entry| Path::new(&entry.path) == current) {
        roots.push(RootEntry {
            name: current.display().to_string(),
            path: current.display().to_string(),
        });
    }

    roots
}

pub async fn info(State(state): State<Arc<AppState>>) -> Response {
    if !state.cfg.local_mode {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "接口不存在" })),
        )
            .into_response();
    }
    let root = state.data_root();
    Json(json!({
        "mode": "local",
        "version": crate::VERSION,
        "platform": std::env::consts::OS,
        "root": root.display().to_string(),
        "roots": local_roots(&root),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct SetRootRequest {
    path: String,
}

pub async fn set_root(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SetRootRequest>,
) -> Result<Response, ApiError> {
    if !state.cfg.local_mode {
        return Err(ApiError::not_found("接口不存在"));
    }
    let requested = PathBuf::from(request.path.trim());
    if !requested.is_absolute() {
        return Err(ApiError::bad("必须是绝对路径"));
    }
    let canonical = tokio::fs::canonicalize(&requested)
        .await
        .map_err(|_| ApiError::bad("路径不存在或无法访问"))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|_| ApiError::bad("路径不存在或无法访问"))?;
    if !metadata.is_dir() {
        return Err(ApiError::bad("路径不是文件夹"));
    }
    state.set_data_root(canonical.clone());
    log(&format!("本地模式切换根目录: {}", canonical.display()));
    Ok(Json(json!({ "ok": true, "root": canonical.display().to_string() })).into_response())
}
