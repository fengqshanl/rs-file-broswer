use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{de::DeserializeOwned, Serialize};

use crate::{
    auth::AuthSession,
    crypto::{self, Envelope},
    error::{log, ApiError},
};

pub const API_BODY_LIMIT: usize = 128 * 1024;

pub async fn read_encrypted<T: DeserializeOwned>(
    session: &AuthSession,
    aad: &str,
    body: Body,
) -> Result<T, ApiError> {
    let raw = axum::body::to_bytes(body, API_BODY_LIMIT)
        .await
        .map_err(|_| ApiError::too_large())?;
    let envelope: Envelope =
        serde_json::from_slice(&raw).map_err(|_| ApiError::bad("请求格式错误"))?;
    let nonce = crypto::envelope_nonce(&envelope).map_err(|_| ApiError::bad("请求格式错误"))?;
    // AAD binds a ciphertext to its endpoint but not to a point in time. Each
    // envelope has a fresh random nonce, so a second delivery of the same
    // bytes is a replay and must not reach the handler.
    let fresh = session
        .replay
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .accept(nonce);
    if !fresh {
        return Err(ApiError::conflict("重复的请求已被拒绝，请重试"));
    }
    crypto::decrypt_envelope(&session.key, aad, &envelope).map_err(|err| {
        log(&format!("API 负载解密失败: {err}"));
        ApiError::bad("请求解密失败")
    })
}

pub fn encrypted_response<T: Serialize>(
    session: &AuthSession,
    aad: &str,
    value: T,
) -> Result<Response, ApiError> {
    let body = crypto::encrypt_envelope(&session.key, aad, &value)
        .map_err(|err| ApiError::internal(err.to_string()))?;
    Ok((StatusCode::OK, Json(body)).into_response())
}
