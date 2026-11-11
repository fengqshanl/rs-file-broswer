use std::{net::IpAddr, path::PathBuf};

use anyhow::{Context, Result};
use rcgen::{Certificate, CertificateParams, DnType, SanType};

use crate::config::Config;

/// Reuses an existing certificate if present; otherwise creates a self-signed
/// certificate whose SANs cover loopback plus every configured extra IP/DNS.
pub fn ensure_self_signed(cfg: &Config) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(&cfg.tls_dir)
        .with_context(|| format!("无法创建 TLS 目录: {}", cfg.tls_dir.display()))?;

    let cert_path = cfg.tls_dir.join("cert.pem");
    let key_path = cfg.tls_dir.join("key.pem");
    if cert_path.exists() && key_path.exists() {
        return Ok((cert_path, key_path));
    }

    let mut dns_names = vec!["localhost".to_string()];
    for name in &cfg.tls_extra_dns {
        if !dns_names.iter().any(|existing| existing == name) {
            dns_names.push(name.clone());
        }
    }

    let mut params = CertificateParams::new(dns_names);
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    params
        .distinguished_name
        .push(DnType::CommonName, "AgentGod Files");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "AgentGod");

    let mut ips: Vec<IpAddr> = vec![
        IpAddr::from([127, 0, 0, 1]),
        IpAddr::from([0u16, 0, 0, 0, 0, 0, 0, 1]),
    ];
    ips.extend(cfg.tls_extra_ips.iter().copied());
    let mut seen_ips = std::collections::HashSet::new();
    for ip in ips {
        if seen_ips.insert(ip) {
            params.subject_alt_names.push(SanType::IpAddress(ip));
        }
    }

    let certificate = Certificate::from_params(params).context("生成自签名 TLS 证书失败")?;

    std::fs::write(
        &cert_path,
        certificate.serialize_pem().context("编码 TLS 证书失败")?,
    )
    .context("写入 TLS 证书失败")?;
    std::fs::write(&key_path, certificate.serialize_private_key_pem())
        .context("写入 TLS 私钥失败")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .context("设置 TLS 私钥权限失败")?;
    }

    Ok((cert_path, key_path))
}
