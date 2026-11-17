//! On-demand image thumbnails for the grid layout.
//!
//! The browser cannot decode a remote file without downloading all of it, so a
//! gallery of phone photos would mean hundreds of megabytes just to show a
//! contact sheet. This module decodes a bounded number of pixels on a blocking
//! thread, downsizes, re-encodes, and caches the result on disk, so a revisited
//! folder costs one small read per tile.
//!
//! Thumbnails travel through the same encrypted envelope as every other API
//! response: the frontend turns the base64 payload into an object URL.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use axum::{
    body::Body,
    extract::{OriginalUri, State},
    http::Method,
    response::Response,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, DynamicImage, ImageFormat, ImageReader, RgbImage};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    api::{encrypted_response, read_encrypted},
    auth::AuthSession,
    crypto,
    error::{log, ApiError},
    files::resolve_file,
    state::AppState,
};

const DEFAULT_EDGE: u32 = 256;
const MAX_EDGE: u32 = 512;
const MIN_EDGE: u32 = 64;
/// Bigger sources are skipped: decoding them would cost more memory than a
/// thumbnail is worth, and the tile falls back to a file icon.
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
/// ~80 megapixels. Above this a single decode can exhaust the server.
const MAX_SOURCE_PIXELS: u64 = 80_000_000;
const JPEG_QUALITY: u8 = 82;

#[derive(Deserialize)]
struct ThumbnailRequest {
    path: String,
    #[serde(default)]
    size: Option<u32>,
}

/// Extensions the `image` crate can actually decode. SVG is deliberately absent:
/// it needs a full renderer, and the UI shows its icon instead.
fn decodable(name: &str) -> bool {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "jfif" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico"
    )
}

pub async fn thumbnail(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    let aad = crypto::request_aad(&method, &uri);
    let request: ThumbnailRequest = read_encrypted(&session, &aad, body).await?;
    let (path, _clean, name, _size) = resolve_file(&state, &request.path).await?;
    if !decodable(&name) {
        return Err(ApiError::bad("该文件不支持缩略图"));
    }
    let edge = request
        .size
        .unwrap_or(DEFAULT_EDGE)
        .clamp(MIN_EDGE, MAX_EDGE);

    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| ApiError::not_found("文件不存在"))?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(ApiError::bad("图片过大，无法生成缩略图"));
    }

    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let cache_dir = state.cfg.state_dir.join("thumbnails");
    let key = cache_key(&path, metadata.len(), modified, edge);
    let cache_path = cache_dir.join(format!("{key}.jpg"));

    if let Ok(bytes) = tokio::fs::read(&cache_path).await {
        return encrypted_response(
            &session,
            &aad,
            json!({
                "data": STANDARD.encode(&bytes),
                "cached": true,
            }),
        );
    }

    let permit = state
        .preview_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("缩略图并发控制器已关闭"))?;
    let source = path.clone();
    let (bytes, width, height) =
        tokio::task::spawn_blocking(move || render(&source, edge))
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?
            .map_err(|err| ApiError::bad(err))?;
    drop(permit);

    // The cache is best effort: a read-only state directory must not turn a
    // successful thumbnail into an error.
    if tokio::fs::create_dir_all(&cache_dir).await.is_ok() {
        if let Err(err) = tokio::fs::write(&cache_path, &bytes).await {
            log(&format!("缩略图缓存写入失败: {err}"));
        }
    }

    encrypted_response(
        &session,
        &aad,
        json!({
            "data": STANDARD.encode(&bytes),
            "width": width,
            "height": height,
            "cached": false,
        }),
    )
}

fn cache_key(path: &Path, len: u64, modified: u64, edge: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(len.to_le_bytes());
    hasher.update(modified.to_le_bytes());
    hasher.update(edge.to_le_bytes());
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

/// Decodes, downsizes and re-encodes. Returns the JPEG bytes and pixel size.
fn render(path: &PathBuf, edge: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let bytes = std::fs::read(path).map_err(|err| format!("读取图片失败: {err}"))?;
    let format = image::guess_format(&bytes).map_err(|_| "无法识别的图片格式".to_string())?;
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Bmp
            | ImageFormat::Tiff
            | ImageFormat::Ico
    ) {
        return Err("该图片格式不支持缩略图".to_string());
    }

    // Read the header first so a decompression bomb cannot allocate gigabytes.
    let (source_width, source_height) =
        ImageReader::with_format(Cursor::new(&bytes), format)
            .into_dimensions()
            .map_err(|_| "无法解析图片尺寸".to_string())?;
    if u64::from(source_width) * u64::from(source_height) > MAX_SOURCE_PIXELS {
        return Err("图片分辨率过高，已跳过缩略图".to_string());
    }

    let image = image::load_from_memory_with_format(&bytes, format)
        .map_err(|_| "图片解码失败".to_string())?;
    let thumb = image.thumbnail(edge, edge);
    let (width, height) = (thumb.width(), thumb.height());

    // JPEG has no alpha channel, so transparency is flattened onto white —
    // otherwise every transparent PNG would turn into a black tile.
    let flat = flatten(&thumb);
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    encoder
        .encode_image(&flat)
        .map_err(|err| format!("缩略图编码失败: {err}"))?;
    Ok((out, width, height))
}

fn flatten(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = u32::from(pixel[3]);
        let inverse = 255 - alpha;
        let blend = |channel: u8| -> u8 {
            ((u32::from(channel) * alpha + 255 * inverse) / 255) as u8
        };
        out.put_pixel(
            x,
            y,
            image::Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])]),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn cache_keys_differ_per_size_and_mtime() {
        let path = PathBuf::from("/tmp/a.png");
        assert_eq!(cache_key(&path, 10, 100, 256), cache_key(&path, 10, 100, 256));
        assert_ne!(cache_key(&path, 10, 100, 256), cache_key(&path, 10, 100, 512));
        assert_ne!(cache_key(&path, 10, 100, 256), cache_key(&path, 11, 100, 256));
    }

    #[test]
    fn flattens_transparency_onto_white() {
        let mut rgba = RgbaImage::new(2, 1);
        rgba.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        rgba.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        let flat = flatten(&DynamicImage::ImageRgba8(rgba));
        assert_eq!(flat.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(flat.get_pixel(1, 0).0, [0, 0, 0]);
    }

    #[test]
    fn rejects_formats_without_a_decoder() {
        assert!(decodable("photo.JPG"));
        assert!(decodable("icon.webp"));
        assert!(!decodable("vector.svg"));
        assert!(!decodable("notes.txt"));
    }
}
