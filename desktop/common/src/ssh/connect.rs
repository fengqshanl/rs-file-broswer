//! SSH connection setup: transport, authentication, host-key verification and
//! the SFTP subsystem.
//!
//! Host keys follow trust-on-first-use: the caller passes the pin stored in
//! `known_hosts.json`; an unknown key aborts the handshake and reports the
//! fingerprint so the UI can ask the user, and a different key reports a
//! mismatch and is refused outright.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use russh::{
    client,
    keys::{load_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};
use russh_sftp::client::SftpSession;
use serde::Serialize;

use super::SshError;
use crate::config::SshAuth;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const KEEPALIVE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct HostKeyInfo {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// Base64 of the SSH wire-format public key blob.
    pub key: String,
    /// `SHA256:...` as printed by `ssh-keygen -lf`.
    pub fingerprint: String,
}

pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    pub key_path: Option<String>,
    pub password: Option<String>,
    pub passphrase: Option<String>,
    /// Pin from `known_hosts.json`, if any.
    pub pinned: Option<HostKeyInfo>,
}

pub struct SshSession {
    /// Kept alive for the lifetime of the SFTP session.
    handle: client::Handle<ClientHandler>,
    pub sftp: Arc<SftpSession>,
    pub host_key: HostKeyInfo,
}

impl SshSession {
    /// True once the transport closed (server restart, network drop, …).
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }
}

struct ClientHandler {
    host: String,
    port: u16,
    pinned: Option<HostKeyInfo>,
    seen: Arc<Mutex<Option<HostKeyInfo>>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = server_public_key.public_key();
        let blob = match key.to_bytes() {
            Ok(blob) => blob,
            Err(_) => return Ok(false),
        };
        let info = HostKeyInfo {
            host: self.host.clone(),
            port: self.port,
            algorithm: key.algorithm().to_string(),
            key: STANDARD.encode(blob),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        };
        *self.seen.lock().unwrap_or_else(|err| err.into_inner()) = Some(info.clone());
        Ok(match &self.pinned {
            Some(pin) => pin.key == info.key,
            None => false,
        })
    }
}

pub async fn connect(params: ConnectParams) -> Result<SshSession, SshError> {
    let seen = Arc::new(Mutex::new(None));
    let handler = ClientHandler {
        host: params.host.clone(),
        port: params.port,
        pinned: params.pinned.clone(),
        seen: seen.clone(),
    };
    let config = client::Config {
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    };

    let connect_result = tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(
            Arc::new(config),
            (params.host.as_str(), params.port),
            handler,
        ),
    )
    .await;

    let mut handle = match connect_result {
        Err(_) => {
            return Err(SshError::new(
                "ssh_timeout",
                format!(
                    "连接 {}:{} 超时（{} 秒）",
                    params.host,
                    params.port,
                    CONNECT_TIMEOUT.as_secs()
                ),
            ))
        }
        Ok(Err(err)) => {
            let observed = seen
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            return Err(match observed {
                Some(info) if params.pinned.is_none() => SshError::host_key_unknown(info),
                Some(info) => SshError::host_key_mismatch(
                    params.pinned.as_ref().map(|pin| pin.fingerprint.clone()),
                    info,
                ),
                None => SshError::new(
                    "ssh_connect_failed",
                    format!("无法连接 {}:{}：{err}", params.host, params.port),
                ),
            });
        }
        Ok(Ok(handle)) => handle,
    };

    authenticate(&mut handle, &params).await?;
    // Clone before the `if let`: a temporary guard in the scrutinee would live
    // through the branch, and a std::sync guard is not `Send`.
    let observed = seen
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    if let Some(info) = observed {
        return finish(handle, info).await;
    }
    Err(SshError::new("ssh_connect_failed", "服务器未提供主机密钥"))
}

async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    params: &ConnectParams,
) -> Result<(), SshError> {
    let user = params.username.clone();
    match params.auth {
        SshAuth::Password => {
            let password = params
                .password
                .clone()
                .ok_or_else(|| SshError::new("ssh_auth_failed", "没有保存的 SSH 密码"))?;
            let result = handle
                .authenticate_password(user.clone(), password.clone())
                .await
                .map_err(|err| SshError::new("ssh_auth_failed", format!("认证失败：{err}")))?;
            if result.success() {
                return Ok(());
            }
            // Many server images only offer keyboard-interactive for PAM.
            if keyboard_interactive(handle, &user, &password).await? {
                return Ok(());
            }
            Err(SshError::new("ssh_auth_failed", "SSH 用户名或密码错误"))
        }
        SshAuth::Key => {
            let path = params
                .key_path
                .as_deref()
                .map(expand_home)
                .ok_or_else(|| SshError::new("ssh_auth_failed", "没有配置私钥路径"))?;
            let key = load_secret_key(&path, params.passphrase.as_deref()).map_err(|err| {
                SshError::new(
                    "ssh_auth_failed",
                    format!("无法读取私钥 {}：{err}", path.display()),
                )
            })?;
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg);
            let result = handle
                .authenticate_publickey(user, key)
                .await
                .map_err(|err| SshError::new("ssh_auth_failed", format!("认证失败：{err}")))?;
            if result.success() {
                Ok(())
            } else {
                Err(SshError::new(
                    "ssh_auth_failed",
                    "私钥被服务器拒绝（请确认公钥已加入 authorized_keys）",
                ))
            }
        }
    }
}

async fn keyboard_interactive(
    handle: &mut client::Handle<ClientHandler>,
    user: &str,
    password: &str,
) -> Result<bool, SshError> {
    use russh::client::KeyboardInteractiveAuthResponse;
    let mut response = handle
        .authenticate_keyboard_interactive_start(user.to_string(), None)
        .await
        .map_err(|err| SshError::new("ssh_auth_failed", format!("认证失败：{err}")))?;
    for _ in 0..4 {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|err| SshError::new("ssh_auth_failed", format!("认证失败：{err}")))?;
            }
        }
    }
    Ok(false)
}

async fn finish(
    handle: client::Handle<ClientHandler>,
    host_key: HostKeyInfo,
) -> Result<SshSession, SshError> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|err| SshError::new("ssh_connect_failed", format!("无法打开 SSH 通道：{err}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|err| SshError::new("ssh_connect_failed", format!("服务器不支持 SFTP：{err}")))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|err| SshError::new("ssh_connect_failed", format!("SFTP 初始化失败：{err}")))?;
    Ok(SshSession {
        handle,
        sftp: Arc::new(sftp),
        host_key,
    })
}

/// Expands a leading `~` so users can paste `~/.ssh/id_ed25519`.
pub fn expand_home(input: &str) -> PathBuf {
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(input)
}
