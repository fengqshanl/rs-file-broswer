//! SSH/SFTP targets for the desktop app.
//!
//! Each SSH target is served by its own loopback HTTP service whose contract is
//! identical to the local service, so the companion's proxy and the whole
//! frontend work unchanged.

pub mod connect;
pub mod fs;
pub mod service;
pub mod tar;

use serde::Serialize;

pub use connect::HostKeyInfo;

/// Structured failure surfaced to the UI. `code` drives the frontend flow
/// (host-key confirmation, credential prompt, retry).
#[derive(Clone, Debug, Serialize)]
pub struct SshError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_key: Option<HostKeyInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_fingerprint: Option<String>,
}

impl SshError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            host_key: None,
            previous_fingerprint: None,
        }
    }

    pub fn host_key_unknown(info: HostKeyInfo) -> Self {
        Self {
            code: "host_key_unknown".to_string(),
            message: format!(
                "首次连接 {}:{}，请确认主机指纹 {}",
                info.host, info.port, info.fingerprint
            ),
            host_key: Some(info),
            previous_fingerprint: None,
        }
    }

    pub fn host_key_mismatch(previous: Option<String>, info: HostKeyInfo) -> Self {
        Self {
            code: "host_key_mismatch".to_string(),
            message: format!(
                "{}:{} 的主机密钥已变更（当前 {}），出于安全考虑已拒绝连接",
                info.host, info.port, info.fingerprint
            ),
            host_key: Some(info),
            previous_fingerprint: previous,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("ssh_internal", message)
    }
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SshError {}

#[cfg(test)]
mod compile_tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn session_types_are_thread_safe() {
        assert_send_sync::<connect::SshSession>();
        assert_send_sync::<fs::SftpFs>();
        assert_send_sync::<service::SshState>();
    }
}
