use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use argon2::password_hash::PasswordHash;
use serde::{Deserialize, Serialize};

use crate::{config::Config, crypto, error::log};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password_hash: String,
}

impl Credentials {
    /// Local desktop mode never authenticates, so no usable password exists.
    pub fn local() -> (Self, PathBuf) {
        (
            Credentials {
                username: "local".to_string(),
                password_hash: "!".to_string(),
            },
            PathBuf::new(),
        )
    }

    /// The credentials file wins over ADMIN_USER / ADMIN_PASSWORD after first
    /// boot so that changes made in the web UI survive container restarts.
    pub fn load_or_init(cfg: &Config) -> Result<(Self, PathBuf)> {
        let path = cfg.state_dir.join("credentials.json");
        if path.exists() {
            let creds = Self::load_from(&path)?;
            log("已从 state/credentials.json 加载管理员凭据");
            Ok((creds, path))
        } else {
            let password_hash = match (&cfg.admin_password_hash, &cfg.admin_password) {
                (Some(hash), _) => {
                    PasswordHash::new(hash).map_err(|_| {
                        anyhow::anyhow!("ADMIN_PASSWORD_HASH 不是有效的 Argon2 PHC 字符串")
                    })?;
                    hash.clone()
                }
                (None, Some(password)) => crypto::hash_password(password)?,
                (None, None) => bail!("缺少管理员密码"),
            };
            validate_username(&cfg.admin_user)
                .map_err(|message| anyhow::anyhow!("ADMIN_USER 无效: {message}"))?;
            let creds = Credentials {
                username: cfg.admin_user.clone(),
                password_hash,
            };
            creds.save(&path)?;
            log("已根据环境变量初始化管理员凭据并写入 state/credentials.json");
            Ok((creds, path))
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("读取凭据文件失败: {}", path.display()))?;
        let creds: Credentials = serde_json::from_str(&raw)
            .context("解析凭据文件失败，请检查 state/credentials.json")?;
        validate_username(&creds.username)
            .map_err(|message| anyhow::anyhow!("凭据文件中的用户名无效: {message}"))?;
        PasswordHash::new(&creds.password_hash)
            .map_err(|_| anyhow::anyhow!("凭据文件中的密码哈希无效"))?;
        Ok(creds)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let data = serde_json::to_vec_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        let mut file = File::create(&tmp)
            .with_context(|| format!("创建临时凭据文件失败: {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
                .context("设置凭据文件权限失败")?;
        }
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path).with_context(|| format!("保存凭据文件失败: {}", path.display()))?;
        Ok(())
    }
}

pub fn validate_username(username: &str) -> std::result::Result<(), String> {
    let length = username.chars().count();
    if length == 0 || length > 64 {
        return Err("用户名长度需为 1–64 个字符".to_string());
    }
    if username.trim() != username {
        return Err("用户名首尾不能包含空白字符".to_string());
    }
    if username.chars().any(char::is_control) || username.contains('/') {
        return Err("用户名包含非法字符".to_string());
    }
    Ok(())
}

pub fn validate_password(password: &str) -> std::result::Result<(), String> {
    let length = password.chars().count();
    if !(8..=256).contains(&length) {
        return Err("密码长度需为 8–256 个字符".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_roundtrip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.json");
        let credentials = Credentials {
            username: "测试用户".to_string(),
            password_hash: crypto::hash_password("correct horse battery staple").unwrap(),
        };
        credentials.save(&path).unwrap();
        let loaded = Credentials::load_from(&path).unwrap();
        assert_eq!(loaded.username, credentials.username);
        assert!(crypto::verify_password_hash(
            &loaded.password_hash,
            "correct horse battery staple"
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn username_validation() {
        assert!(validate_username("admin").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username(" admin").is_err());
        assert!(validate_username("a/b").is_err());
        assert!(validate_username(&"x".repeat(65)).is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("short").is_err());
    }
}
