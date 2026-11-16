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

/// Canonical key used to compare roots without `\\?\` prefixes or Windows
/// case/separator differences.
fn path_key(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text = canonical.to_string_lossy().replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_string();
    }
    #[cfg(windows)]
    text.make_ascii_lowercase();
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn push_root(roots: &mut Vec<RootEntry>, name: String, path: PathBuf) {
    let key = path_key(&path);
    if roots
        .iter()
        .any(|entry| path_key(Path::new(&entry.path)) == key)
    {
        return;
    }
    roots.push(RootEntry {
        name,
        path: path.display().to_string(),
    });
}

/// Roots offered by the desktop "local" companion: the user's home, the
/// filesystem root (or every drive on Windows), and the current root if it is
/// not one of those.
pub fn local_roots(current: &Path) -> Vec<RootEntry> {
    let mut roots = Vec::new();

    if let Some(home) = dirs::home_dir() {
        push_root(&mut roots, "主目录".to_string(), home);
    }

    #[cfg(windows)]
    {
        for letter in b'C'..=b'Z' {
            let path = format!("{}:\\", letter as char);
            if Path::new(&path).exists() {
                push_root(
                    &mut roots,
                    format!("{}: 盘", letter as char),
                    PathBuf::from(path),
                );
            }
        }
    }
    #[cfg(not(windows))]
    {
        push_root(&mut roots, "根目录".to_string(), PathBuf::from("/"));
    }

    push_root(
        &mut roots,
        current.display().to_string(),
        current.to_path_buf(),
    );

    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_are_unique_and_include_the_current_root() {
        let current = std::env::current_dir().expect("cwd");
        let roots = local_roots(&current);
        let keys: Vec<String> = roots
            .iter()
            .map(|entry| path_key(Path::new(&entry.path)))
            .collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(keys.len(), unique.len(), "duplicate roots: {keys:?}");
        assert!(keys.contains(&path_key(&current)));
    }
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
