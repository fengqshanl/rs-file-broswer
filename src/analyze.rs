//! Storage analysis for the desktop "analysis" extension.
//!
//! The panel needs three things the plain listing cannot give: how much each
//! direct child really occupies (recursively), which file families dominate the
//! folder, and how full the volume is. One walk answers all of them, so a folder
//! is scanned once instead of once per child.
//!
//! Every scan is bounded by a wall-clock budget and an entry budget. A folder
//! that exceeds either is reported with `truncated: true` rather than blocking a
//! worker thread for minutes on a multi-terabyte share.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{OriginalUri, State},
    http::Method,
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    api::{encrypted_response, read_encrypted},
    auth::AuthSession,
    crypto,
    error::ApiError,
    files::{is_hidden, logical_path, resolve_dir},
    state::AppState,
};

/// A completed scan is reused for this long. Mutations clear the cache
/// immediately; the TTL only bounds changes made outside the app.
const ANALYSIS_TTL: Duration = Duration::from_secs(45);
/// Wall-clock ceiling for one scan. Long enough for a large folder, short
/// enough that the UI keeps its "scanning" state meaningful.
const ANALYSIS_BUDGET: Duration = Duration::from_secs(20);
const MAX_ANALYSIS_ENTRIES: usize = 400_000;
const MAX_ANALYSIS_DEPTH: usize = 48;
const MAX_CACHED_ANALYSES: usize = 24;
/// The ranking only needs the biggest entries. Returning every child of a
/// 50k-file folder would cost more to encrypt and render than it explains.
const MAX_ANALYSIS_CHILDREN: usize = 400;

#[derive(Deserialize)]
struct AnalyzeRequest {
    #[serde(default)]
    path: String,
}

/// One row of the analysis: a direct child of the scanned folder with its whole
/// subtree already folded in.
#[derive(Clone, Debug, Serialize)]
pub struct ChildStat {
    name: String,
    is_dir: bool,
    hidden: bool,
    /// Recursive byte total.
    size: u64,
    /// Recursive file count.
    files: u64,
    /// Recursive directory count.
    dirs: u64,
    modified: u64,
    /// File family (`image`, `video`, …) or `folder` for directories.
    kind: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct CategoryStat {
    kind: &'static str,
    size: u64,
    files: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    path: String,
    total_size: u64,
    total_files: u64,
    total_dirs: u64,
    /// The walk hit the time or entry budget; totals are a partial sum.
    truncated: bool,
    duration_ms: u64,
    free_space: Option<u64>,
    total_space: Option<u64>,
    children: Vec<ChildStat>,
    /// Direct children before the ranking was capped.
    child_total: usize,
    /// `children` holds only the largest entries.
    children_truncated: bool,
    categories: Vec<CategoryStat>,
}

pub struct CachedAnalysis {
    pub analysis: Arc<Analysis>,
    pub built_at: Instant,
}

#[derive(Serialize)]
pub struct Volume {
    name: String,
    path: String,
    total: Option<u64>,
    free: Option<u64>,
    used: Option<u64>,
}

#[derive(Deserialize)]
struct VolumesRequest {
    #[serde(default)]
    path: String,
}

/// Maps an extension to the family used by the analysis legend.
pub fn file_kind(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "jfif" | "gif" | "webp" | "avif" | "bmp" | "ico" | "svg"
        | "heic" | "heif" | "tif" | "tiff" | "raw" | "cr2" | "nef" | "arw" | "dng" => "image",
        "mp4" | "m4v" | "webm" | "ogv" | "mov" | "mkv" | "avi" | "wmv" | "flv" | "mpg"
        | "mpeg" | "m2ts" | "3gp" => "video",
        "mp3" | "wav" | "ogg" | "oga" | "m4a" | "aac" | "flac" | "wma" | "opus" | "aiff"
        | "mid" | "midi" => "audio",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "markdown"
        | "rtf" | "odt" | "ods" | "odp" | "csv" | "tsv" | "epub" | "mobi" | "pages"
        | "numbers" | "key" => "document",
        "rs" | "py" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "go" | "java" | "c"
        | "h" | "cpp" | "hpp" | "cc" | "cs" | "rb" | "php" | "sh" | "bash" | "zsh" | "ps1"
        | "sql" | "html" | "htm" | "css" | "scss" | "less" | "json" | "yaml" | "yml" | "toml"
        | "xml" | "lock" | "vue" | "svelte" | "swift" | "kt" | "scala" | "lua" | "r" => "code",
        "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "iso" | "dmg"
        | "cab" | "apk" | "deb" | "rpm" | "msi" => "archive",
        "exe" | "dll" | "so" | "dylib" | "bin" | "sys" | "app" | "jar" | "wasm" => "binary",
        _ => "other",
    }
}

pub async fn analyze(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: AnalyzeRequest = read_encrypted(&session, &aad, body).await?;
    let (directory, clean) = resolve_dir(&state, &request.path).await?;

    if let Some(cached) = state.analyses.get(&directory) {
        if cached.built_at.elapsed() < ANALYSIS_TTL {
            return encrypted_response(&session, &aad, cached.analysis.as_ref());
        }
    }

    let root = directory.clone();
    let display = logical_path(&clean);
    let tmp_root = state.cfg.tmp_root.clone();
    let analysis = tokio::task::spawn_blocking(move || analyze_dir(&root, &tmp_root, display))
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?
        .map_err(|err| ApiError::internal(err.to_string()))?;

    if state.analyses.len() >= MAX_CACHED_ANALYSES {
        state.analyses.clear();
    }
    state.analyses.insert(
        directory,
        CachedAnalysis {
            analysis: analysis.clone(),
            built_at: Instant::now(),
        },
    );

    encrypted_response(&session, &aad, analysis.as_ref())
}

/// Volumes reachable from the current root, so the panel can show how full each
/// disk is without walking it first.
pub async fn volumes(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: VolumesRequest = read_encrypted(&session, &aad, body).await?;
    let (directory, _clean) = resolve_dir(&state, &request.path).await?;
    let current = directory.display().to_string();
    let roots = crate::app::local_roots(&directory);

    let list: Vec<Volume> = roots
        .into_iter()
        // `local_roots` appends the current directory so the path bar can jump
        // there; for volumes that entry would be a deep path, not a disk.
        .filter(|root| root.path != current)
        .map(|root| {
            let path = PathBuf::from(&root.path);
            let total = fs2::total_space(&path).ok();
            let free = fs2::available_space(&path).ok();
            let used = match (total, free) {
                (Some(total), Some(free)) => Some(total.saturating_sub(free)),
                _ => None,
            };
            Volume {
                name: root.name,
                path: root.path,
                total,
                free,
                used,
            }
        })
        .collect();

    encrypted_response(&session, &aad, json!({ "volumes": list }))
}

fn analyze_dir(root: &Path, tmp_root: &Path, display: String) -> io::Result<Arc<Analysis>> {
    let started = Instant::now();
    let mut children: Vec<ChildStat> = Vec::new();
    let mut categories: HashMap<&'static str, (u64, u64)> = HashMap::new();
    let mut total_size = 0u64;
    let mut total_files = 0u64;
    let mut total_dirs = 0u64;
    let mut seen = 0usize;
    let mut truncated = false;

    // Depth-first over subdirectories only; every file folds into the direct
    // child it belongs to, which is exactly what the ranking rows show.
    let mut stack: Vec<(PathBuf, usize, usize)> = Vec::new();

    let reader = std::fs::read_dir(root)?;
    for item in reader {
        if seen >= MAX_ANALYSIS_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(item) = item else { continue };
        let path = item.path();
        if path == tmp_root || path.starts_with(tmp_root) {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // Symlinks are skipped: following them can loop and double-count bytes.
        if metadata.file_type().is_symlink() {
            continue;
        }
        seen += 1;
        let name = item.file_name().to_string_lossy().to_string();
        let hidden = is_hidden(&name, &metadata);
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);

        if metadata.is_dir() {
            total_dirs += 1;
            children.push(ChildStat {
                name,
                is_dir: true,
                hidden,
                size: 0,
                files: 0,
                dirs: 0,
                modified,
                kind: "folder",
            });
            let owner = children.len() - 1;
            stack.push((path, 1, owner));
        } else {
            let size = metadata.len();
            total_size = total_size.saturating_add(size);
            total_files += 1;
            let kind = file_kind(&name);
            let entry = categories.entry(kind).or_insert((0, 0));
            entry.0 = entry.0.saturating_add(size);
            entry.1 += 1;
            children.push(ChildStat {
                name,
                is_dir: false,
                hidden,
                size,
                files: 1,
                dirs: 0,
                modified,
                kind,
            });
        }
    }

    let mut visited_dirs = total_dirs;
    'walk: while let Some((directory, depth, owner)) = stack.pop() {
        if depth > MAX_ANALYSIS_DEPTH {
            truncated = true;
            continue;
        }
        let Ok(reader) = std::fs::read_dir(&directory) else {
            continue;
        };
        for item in reader {
            if seen >= MAX_ANALYSIS_ENTRIES || started.elapsed() > ANALYSIS_BUDGET {
                truncated = true;
                break 'walk;
            }
            let Ok(item) = item else { continue };
            let path = item.path();
            if path == tmp_root || path.starts_with(tmp_root) {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            seen += 1;
            if metadata.is_dir() {
                visited_dirs += 1;
                children[owner].dirs += 1;
                stack.push((path, depth + 1, owner));
            } else {
                let size = metadata.len();
                children[owner].size = children[owner].size.saturating_add(size);
                children[owner].files += 1;
                total_size = total_size.saturating_add(size);
                total_files += 1;
                let kind = file_kind(&item.file_name().to_string_lossy());
                let entry = categories.entry(kind).or_insert((0, 0));
                entry.0 = entry.0.saturating_add(size);
                entry.1 += 1;
            }
        }
    }
    total_dirs = visited_dirs;

    children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    let child_total = children.len();
    if children.len() > MAX_ANALYSIS_CHILDREN {
        children.truncate(MAX_ANALYSIS_CHILDREN);
    }
    let children_truncated = child_total > children.len();

    let mut categories: Vec<CategoryStat> = categories
        .into_iter()
        .map(|(kind, (size, files))| CategoryStat { kind, size, files })
        .collect();
    categories.sort_by(|a, b| b.size.cmp(&a.size));

    Ok(Arc::new(Analysis {
        path: display,
        total_size,
        total_files,
        total_dirs,
        truncated,
        duration_ms: started.elapsed().as_millis() as u64,
        free_space: fs2::available_space(root).ok(),
        total_space: fs2::total_space(root).ok(),
        children,
        child_total,
        children_truncated,
        categories,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(dir: &Path) {
        fs::create_dir_all(dir.join("photos/2024")).unwrap();
        fs::create_dir_all(dir.join("docs")).unwrap();
        fs::write(dir.join("photos/a.jpg"), vec![1u8; 1000]).unwrap();
        fs::write(dir.join("photos/2024/b.png"), vec![2u8; 500]).unwrap();
        fs::write(dir.join("docs/report.pdf"), vec![3u8; 200]).unwrap();
        fs::write(dir.join("notes.txt"), vec![4u8; 50]).unwrap();
    }

    #[test]
    fn folders_roll_up_their_subtree() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let tmp = dir.path().join("tmp-unused");
        let analysis = analyze_dir(dir.path(), &tmp, "/".to_string()).unwrap();

        assert_eq!(analysis.total_files, 4);
        assert_eq!(analysis.total_size, 1750);
        assert!(!analysis.truncated);

        let photos = analysis
            .children
            .iter()
            .find(|child| child.name == "photos")
            .expect("photos row");
        assert_eq!(photos.size, 1500);
        assert_eq!(photos.files, 2);
        assert_eq!(photos.dirs, 1);

        let docs = analysis
            .children
            .iter()
            .find(|child| child.name == "docs")
            .unwrap();
        assert_eq!(docs.size, 200);
        assert_eq!(docs.files, 1);

        // Largest first, so the ranking is already sorted for the panel.
        assert_eq!(analysis.children[0].name, "photos");
        let images = analysis
            .categories
            .iter()
            .find(|stat| stat.kind == "image")
            .expect("image category");
        assert_eq!(images.size, 1500);
        assert_eq!(images.files, 2);
    }

    #[test]
    fn kind_mapping_covers_the_common_families() {
        assert_eq!(file_kind("a.JPG"), "image");
        assert_eq!(file_kind("movie.mkv"), "video");
        assert_eq!(file_kind("song.flac"), "audio");
        assert_eq!(file_kind("paper.pdf"), "document");
        assert_eq!(file_kind("main.rs"), "code");
        assert_eq!(file_kind("backup.7z"), "archive");
        assert_eq!(file_kind("setup.exe"), "binary");
        assert_eq!(file_kind("no-extension"), "other");
    }

    #[test]
    fn symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("photos"), dir.path().join("link"))
                .unwrap();
            let tmp = dir.path().join("tmp-unused");
            let analysis = analyze_dir(dir.path(), &tmp, "/".to_string()).unwrap();
            assert_eq!(analysis.total_size, 1750);
            assert!(analysis
                .children
                .iter()
                .all(|child| child.name != "link"));
        }
    }
}
