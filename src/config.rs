use std::{env, net::IpAddr, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: IpAddr,
    pub port: u16,
    pub data_root: PathBuf,
    pub tmp_root: PathBuf,
    pub state_dir: PathBuf,
    pub tls_dir: PathBuf,
    pub admin_user: String,
    pub admin_password: Option<String>,
    pub admin_password_hash: Option<String>,
    pub session_ttl: Duration,
    pub session_max_ttl: Duration,
    pub upload_rate_bps: f64,
    pub download_rate_bps: f64,
    pub max_upload_concurrency: usize,
    pub max_download_concurrency: usize,
    pub max_preview_concurrency: usize,
    pub max_active_uploads: usize,
    pub max_file_bytes: u64,
    pub disk_reserve_bytes: u64,
    pub rsa_bits: usize,
    pub tls_extra_ips: Vec<IpAddr>,
    pub tls_extra_dns: Vec<String>,
    pub login_max_failures: u32,
    pub login_window: Duration,
}

fn env_opt(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    env_opt(name).unwrap_or_else(|| default.to_owned())
}

fn env_parse<T>(name: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env_opt(name) {
        Some(raw) => raw
            .parse()
            .map_err(|err| anyhow::anyhow!("环境变量 {name} 无效: {err}")),
        None => Ok(default),
    }
}

fn env_list<T>(name: &str) -> Result<Vec<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let Some(raw) = env_opt(name) else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse()
                .map_err(|err| anyhow::anyhow!("环境变量 {name} 中的值 '{part}' 无效: {err}"))
        })
        .collect()
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let data_root_raw = PathBuf::from(env_or("DATA_ROOT", "/data/files"));
        let data_root = std::fs::canonicalize(&data_root_raw)
            .with_context(|| format!("数据目录不可用: {}", data_root_raw.display()))?;

        let tmp_raw = env_opt("TMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_root.join(".agentgod-tmp"));
        std::fs::create_dir_all(&tmp_raw)
            .with_context(|| format!("无法创建临时目录: {}", tmp_raw.display()))?;
        let tmp_root = std::fs::canonicalize(&tmp_raw)?;
        if tmp_root == data_root || !tmp_root.starts_with(&data_root) {
            bail!("TMP_ROOT 必须是 DATA_ROOT 内的独立子目录（保证上传完成时原子移动）");
        }

        let state_raw = PathBuf::from(env_or("STATE_DIR", "/data/state"));
        std::fs::create_dir_all(&state_raw)
            .with_context(|| format!("无法创建状态目录: {}", state_raw.display()))?;
        let state_dir = std::fs::canonicalize(&state_raw)?;

        let tls_raw = env_opt("TLS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| state_dir.join("tls"));
        let tls_dir = if tls_raw.exists() {
            std::fs::canonicalize(&tls_raw)?
        } else {
            tls_raw
        };

        let admin_user = env_or("ADMIN_USER", "admin");
        let admin_password = env_opt("ADMIN_PASSWORD");
        let admin_password_hash = env_opt("ADMIN_PASSWORD_HASH");
        if admin_password.is_none() && admin_password_hash.is_none() {
            bail!("必须设置 ADMIN_PASSWORD 或 ADMIN_PASSWORD_HASH");
        }
        if admin_password_hash.is_none()
            && admin_password
                .as_deref()
                .map(str::chars)
                .map(|c| c.count())
                .unwrap_or(0)
                < 8
        {
            bail!("ADMIN_PASSWORD 至少需要 8 个字符");
        }

        let upload_rate_mbps = env_parse::<f64>("UPLOAD_RATE_MBPS", 20.0)?;
        let download_rate_mbps = env_parse::<f64>("DOWNLOAD_RATE_MBPS", 30.0)?;
        if upload_rate_mbps < 0.0 || download_rate_mbps < 0.0 {
            bail!("限速值不能为负数（0 表示不限速）");
        }

        let max_file_size_gb = env_parse::<u64>("MAX_FILE_SIZE_GB", 100)?;
        if max_file_size_gb == 0 {
            bail!("MAX_FILE_SIZE_GB 必须大于 0");
        }
        let max_file_bytes = max_file_size_gb.saturating_mul(1024 * 1024 * 1024);
        let disk_reserve_bytes =
            env_parse::<u64>("DISK_RESERVE_MB", 2048)?.saturating_mul(1024 * 1024);

        let max_upload_concurrency = env_parse::<usize>("MAX_CONCURRENT_UPLOADS", 2)?.clamp(1, 16);
        let max_download_concurrency =
            env_parse::<usize>("MAX_CONCURRENT_DOWNLOADS", 2)?.clamp(1, 16);
        let max_preview_concurrency =
            env_parse::<usize>("MAX_CONCURRENT_PREVIEWS", 4)?.clamp(1, 32);
        let max_active_uploads = env_parse::<usize>("MAX_ACTIVE_UPLOADS", 32)?.clamp(1, 256);

        let session_ttl_secs = env_parse::<u64>("SESSION_TTL_SECS", 7200)?.max(60);
        let session_max_ttl_secs =
            env_parse::<u64>("SESSION_MAX_TTL_SECS", 86_400)?.max(session_ttl_secs);

        let rsa_bits = env_parse::<usize>("RSA_BITS", 2048)?;
        if ![2048, 3072, 4096].contains(&rsa_bits) {
            bail!("RSA_BITS 只能是 2048、3072 或 4096");
        }

        let bind = env_parse::<IpAddr>("BIND", IpAddr::from([0, 0, 0, 0]))?;
        let port = env_parse::<u16>("PORT", 8090)?;

        let login_max_failures = env_parse::<u32>("LOGIN_MAX_FAILURES", 8)?.clamp(1, 100);
        let login_window_secs = env_parse::<u64>("LOGIN_WINDOW_SECS", 600)?.max(30);

        Ok(Self {
            bind,
            port,
            data_root,
            tmp_root,
            state_dir,
            tls_dir,
            admin_user,
            admin_password,
            admin_password_hash,
            session_ttl: Duration::from_secs(session_ttl_secs),
            session_max_ttl: Duration::from_secs(session_max_ttl_secs),
            upload_rate_bps: upload_rate_mbps * 1024.0 * 1024.0,
            download_rate_bps: download_rate_mbps * 1024.0 * 1024.0,
            max_upload_concurrency,
            max_download_concurrency,
            max_preview_concurrency,
            max_active_uploads,
            max_file_bytes,
            disk_reserve_bytes,
            rsa_bits,
            tls_extra_ips: env_list::<IpAddr>("TLS_EXTRA_IPS")?,
            tls_extra_dns: env_list::<String>("TLS_EXTRA_DNS")?,
            login_max_failures,
            login_window: Duration::from_secs(login_window_secs),
        })
    }
}
