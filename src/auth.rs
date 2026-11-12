use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, FromRequestParts, OriginalUri, State},
    http::{header, request::Parts, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rsa::traits::PublicKeyParts;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use crate::{
    api,
    credentials::{self, Credentials},
    crypto::{self, Envelope},
    error::{log, ApiError},
    state::{AppState, Session},
};

const LOGIN_BODY_LIMIT: usize = 32 * 1024;

pub const LOCAL_SESSION_TOKEN: &str = "local";
const LOGIN_TS_SKEW_MS: i64 = 600_000;

pub struct AuthSession {
    pub token: String,
    pub key: [u8; 32],
    pub user: String,
}

pub struct ClientIp(pub Option<IpAddr>);

impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip()),
        ))
    }
}

impl FromRequestParts<Arc<AppState>> for AuthSession {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        if state.cfg.local_mode {
            if let Some(session) = state.sessions.get(LOCAL_SESSION_TOKEN) {
                return Ok(AuthSession {
                    token: LOCAL_SESSION_TOKEN.to_string(),
                    key: session.key,
                    user: session.user.clone(),
                });
            }
            return Err(unauthorized_response());
        }

        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);

        let Some(token) = token else {
            return Err(unauthorized_response());
        };

        let now = Instant::now();
        let (key, user) = {
            let Some(mut session) = state.sessions.get_mut(&token) else {
                return Err(unauthorized_response());
            };
            if now >= session.expires_at || now >= session.hard_expires_at {
                drop(session);
                state.sessions.remove(&token);
                return Err(unauthorized_response());
            }
            session.expires_at = (now + state.cfg.session_ttl).min(session.hard_expires_at);
            (session.key, session.user.clone())
        };

        Ok(AuthSession { token, key, user })
    }
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "未登录或会话已过期" })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct LoginBody {
    ek: String,
    iv: String,
    ct: String,
}

#[derive(Deserialize)]
struct LoginPayload {
    username: String,
    password: String,
    #[serde(default)]
    ts: i64,
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
    expires_in: u64,
    user: String,
}

pub async fn pubkey(State(state): State<Arc<AppState>>) -> Response {
    let mut response = Json(json!({
        "public_key": state.crypto.public_pem,
        "algorithm": "RSA-OAEP-SHA256",
        "session_cipher": "AES-256-GCM",
    }))
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    body: Body,
) -> Result<Response, ApiError> {
    if state.cfg.local_mode {
        return Err(ApiError::bad("本地模式无需登录"));
    }
    let ip = ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    if let Some(remaining) = state.login_guard.blocked_for(ip) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("尝试次数过多，请在 {} 秒后重试", remaining.as_secs().max(1)),
        ));
    }

    let raw = axum::body::to_bytes(body, LOGIN_BODY_LIMIT)
        .await
        .map_err(|_| ApiError::too_large())?;
    let request: LoginBody =
        serde_json::from_slice(&raw).map_err(|_| ApiError::bad("登录请求格式错误"))?;

    let encrypted_key = STANDARD
        .decode(request.ek.as_bytes())
        .map_err(|_| ApiError::bad("登录请求无效"))?;
    if encrypted_key.len() != state.crypto.rsa.size() {
        return Err(ApiError::bad("登录请求无效"));
    }

    let session_key_vec = state
        .crypto
        .rsa
        .decrypt(rsa::Oaep::new::<Sha256>(), &encrypted_key)
        .map_err(|_| ApiError::bad("登录请求无效"))?;
    let session_key: [u8; 32] = session_key_vec
        .try_into()
        .map_err(|_| ApiError::bad("登录请求无效"))?;

    let envelope = Envelope {
        iv: request.iv,
        ct: request.ct,
    };
    let payload: LoginPayload = crypto::decrypt_envelope(&session_key, "login", &envelope)
        .map_err(|_| ApiError::bad("登录请求无效"))?;

    if payload.ts != 0 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        if (now_ms - payload.ts).abs() > LOGIN_TS_SKEW_MS {
            return Err(ApiError::bad("客户端时间偏差过大，请校准设备时间后重试"));
        }
    }

    let LoginPayload {
        username, password, ..
    } = payload;

    let (stored_username, stored_hash) = {
        let credentials = state.credentials.read().await;
        (
            credentials.username.clone(),
            credentials.password_hash.clone(),
        )
    };

    let permit = state
        .login_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("登录并发控制器已关闭"))?;
    let hash = stored_hash;
    let password_for_check = password.clone();
    let password_ok = tokio::task::spawn_blocking(move || {
        crypto::verify_password_hash(&hash, &password_for_check)
    })
    .await
    .unwrap_or(false);
    drop(permit);

    let username_ok = username == stored_username;
    if !password_ok || !username_ok {
        state.login_guard.record_failure(ip);
        log(&format!("登录失败 ip={ip} user={username}"));
        return Err(ApiError::unauthorized("用户名或密码错误"));
    }
    state.login_guard.record_success(ip);

    let token = crypto::random_b64(32);
    let now = Instant::now();
    let expires_at = (now + state.cfg.session_ttl).min(now + state.cfg.session_max_ttl);
    state.sessions.insert(
        token.clone(),
        Session {
            key: session_key,
            user: stored_username.clone(),
            expires_at,
            hard_expires_at: now + state.cfg.session_max_ttl,
        },
    );

    log(&format!("登录成功 ip={ip} user={stored_username}"));

    let response = LoginResponse {
        token,
        expires_in: state.cfg.session_ttl.as_secs(),
        user: stored_username,
    };
    let body = crypto::encrypt_envelope(&session_key, "login-resp", &response)
        .map_err(|err| ApiError::internal(err.to_string()))?;
    Ok((StatusCode::OK, Json(body)).into_response())
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    state.sessions.remove(&session.token);
    crate::preview::revoke_session(&state, &session.token);
    let aad = crypto::request_aad(&method, &uri);
    let body = crypto::encrypt_envelope(&session.key, &aad, &json!({ "ok": true }))
        .map_err(|err| ApiError::internal(err.to_string()))?;
    Ok((StatusCode::OK, Json(body)).into_response())
}

#[derive(Deserialize)]
struct UpdateCredentialsRequest {
    current_password: String,
    new_username: String,
    #[serde(default)]
    new_password: String,
}

pub async fn update_credentials(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    session: AuthSession,
    method: Method,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, ApiError> {
    if state.cfg.local_mode {
        return Err(ApiError::bad("本地模式不支持修改账户"));
    }
    let aad = crypto::request_aad(&method, &uri);
    let request: UpdateCredentialsRequest = api::read_encrypted(&session, &aad, body).await?;
    let ip = ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    if let Some(remaining) = state.login_guard.blocked_for(ip) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("尝试次数过多，请在 {} 秒后重试", remaining.as_secs().max(1)),
        ));
    }

    credentials::validate_username(&request.new_username).map_err(ApiError::bad)?;
    let change_password = !request.new_password.is_empty();
    if change_password {
        credentials::validate_password(&request.new_password).map_err(ApiError::bad)?;
    }
    if request.current_password.is_empty() {
        return Err(ApiError::bad("请输入当前密码"));
    }

    let _update_guard = state.credentials_update.lock().await;
    let current = state.credentials.read().await.clone();

    let permit = state
        .login_sem
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("登录并发控制器已关闭"))?;
    let current_hash = current.password_hash.clone();
    let current_password = request.current_password.clone();
    let verified = tokio::task::spawn_blocking(move || {
        crypto::verify_password_hash(&current_hash, &current_password)
    })
    .await
    .unwrap_or(false);
    drop(permit);

    if !verified {
        state.login_guard.record_failure(ip);
        log(&format!(
            "账户修改失败：当前密码错误 ip={ip} user={}",
            session.user
        ));
        return Err(ApiError::unauthorized("当前密码错误"));
    }
    state.login_guard.record_success(ip);

    let new_hash = if change_password {
        let password = request.new_password.clone();
        let permit = state
            .login_sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ApiError::internal("登录并发控制器已关闭"))?;
        let result = tokio::task::spawn_blocking(move || crypto::hash_password(&password)).await;
        drop(permit);
        match result {
            Ok(Ok(hash)) => hash,
            Ok(Err(err)) => return Err(ApiError::internal(err.to_string())),
            Err(err) => return Err(ApiError::internal(err.to_string())),
        }
    } else {
        current.password_hash.clone()
    };

    let updated = Credentials {
        username: request.new_username.clone(),
        password_hash: new_hash,
    };
    let path = state.credentials_path.clone();
    let to_save = updated.clone();
    let save_result = tokio::task::spawn_blocking(move || to_save.save(&path))
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;
    save_result.map_err(|err| ApiError::internal(err.to_string()))?;

    *state.credentials.write().await = updated;

    // Invalidate every other session; keep the caller signed in so in-flight
    // transfers owned by this session keep working.
    let current_token = session.token.clone();
    state.sessions.retain(|token, _| token == &current_token);
    crate::preview::revoke_other_sessions(&state, &current_token);
    if let Some(mut active) = state.sessions.get_mut(&current_token) {
        active.user = request.new_username.clone();
    }

    log(&format!(
        "账户信息已更新 user={} password_changed={}",
        request.new_username, change_password
    ));

    api::encrypted_response(
        &session,
        &aad,
        json!({
            "ok": true,
            "user": request.new_username,
            "username_changed": request.new_username != current.username,
            "password_changed": change_password,
        }),
    )
}

/// Desktop local mode hands the webview its session directly: the companion
/// server only listens on loopback and has no password to ask for.
pub async fn local_login(State(state): State<Arc<AppState>>) -> Result<Response, ApiError> {
    if !state.cfg.local_mode {
        return Err(ApiError::not_found("接口不存在"));
    }
    let Some(session) = state.sessions.get(LOCAL_SESSION_TOKEN) else {
        return Err(ApiError::internal("本地会话未初始化"));
    };
    let body = json!({
        "token": LOCAL_SESSION_TOKEN,
        "key": STANDARD.encode(session.key),
        "user": session.user.clone(),
    });
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
