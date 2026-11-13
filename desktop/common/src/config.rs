//! Persistent target list for the desktop app.
//!
//! Version 2 merges what used to be two per-app files (`servers-local.json` and
//! `servers-client.json`) into one `targets.json` and adds SSH/SFTP targets.
//! Existing installations are migrated on first start; the legacy files are
//! copied to timestamped backups and left untouched so a rollback to 0.1.6
//! still finds its servers.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::known_hosts::now_seconds;

pub const CONFIG_FILENAME: &str = "targets.json";
pub const CONFIG_VERSION: u32 = 2;
const LEGACY_LOCAL: &str = "servers-local.json";
const LEGACY_CLIENT: &str = "servers-client.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuth {
    Password,
    Key,
}

fn default_port() -> u16 {
    22
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpTarget {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub allow_invalid_certs: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SshTarget {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default = "default_auth")]
    pub auth: SshAuth,
    /// Local path to the private key, used when `auth` is `key`.
    #[serde(default)]
    pub key_path: Option<String>,
    /// Remote directory shown first; `None` means the login home directory.
    #[serde(default)]
    pub root: Option<String>,
}

fn default_username() -> String {
    "root".to_string()
}

fn default_auth() -> SshAuth {
    SshAuth::Password
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetEntry {
    Http(HttpTarget),
    Ssh(SshTarget),
}

impl TargetEntry {
    pub fn id(&self) -> &str {
        match self {
            Self::Http(target) => &target.id,
            Self::Ssh(target) => &target.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Http(target) => &target.name,
            Self::Ssh(target) => &target.name,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetsConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub targets: Vec<TargetEntry>,
}

fn default_version() -> u32 {
    CONFIG_VERSION
}

impl Default for TargetsConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            active: None,
            targets: Vec::new(),
        }
    }
}

impl TargetsConfig {
    pub fn find(&self, id: &str) -> Option<&TargetEntry> {
        self.targets.iter().find(|target| target.id() == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut TargetEntry> {
        self.targets.iter_mut().find(|target| target.id() == id)
    }

    /// Points `active` at the local target when the stored one no longer
    /// exists, so the app always has a valid startup target.
    pub fn normalize_active(&mut self, has_local: bool) {
        let valid = self
            .active
            .as_deref()
            .map(|active| (has_local && active == "local") || self.find(active).is_some());
        if !matches!(valid, Some(true)) {
            self.active = if has_local {
                Some("local".to_string())
            } else {
                self.targets.first().map(|target| target.id().to_string())
            };
        }
    }
}

pub fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILENAME)
}

/// Loads `targets.json`, migrating the two legacy per-app files on first run.
/// Returns the config plus human-readable notes for the log.
pub fn load_or_migrate(dir: &Path) -> (TargetsConfig, Vec<String>) {
    let path = config_path(dir);
    let mut notes = Vec::new();
    if let Ok(raw) = std::fs::read(&path) {
        match serde_json::from_slice::<TargetsConfig>(&raw) {
            Ok(mut config) => {
                config.version = CONFIG_VERSION;
                return (config, notes);
            }
            Err(err) => {
                let stamp = now_seconds();
                let backup = dir.join(format!("{CONFIG_FILENAME}.corrupt-{stamp}"));
                let _ = std::fs::rename(&path, &backup);
                notes.push(format!(
                    "targets.json 解析失败（{err}），已备份到 {}，将从旧配置重建",
                    backup.display()
                ));
            }
        }
    }

    let migrated = migrate_legacy(dir, &mut notes);
    save(dir, &migrated).ok();
    (migrated, notes)
}

#[derive(Deserialize)]
struct LegacyConfig {
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    servers: Vec<LegacyServer>,
}

#[derive(Deserialize)]
struct LegacyServer {
    id: String,
    name: String,
    url: String,
    #[serde(default)]
    allow_invalid_certs: bool,
}

fn migrate_legacy(dir: &Path, notes: &mut Vec<String>) -> TargetsConfig {
    let mut config = TargetsConfig::default();
    let mut seen_urls: Vec<String> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    let mut local_count = 0usize;
    let mut client_added = 0usize;
    let mut duplicates = 0usize;

    for (filename, is_primary) in [(LEGACY_LOCAL, true), (LEGACY_CLIENT, false)] {
        let path = dir.join(filename);
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        let Ok(legacy) = serde_json::from_slice::<LegacyConfig>(&raw) else {
            notes.push(format!("{filename} 无法解析，已跳过"));
            continue;
        };
        // Keep a copy for rollback; the original file stays in place so a
        // downgrade to 0.1.6 still works.
        let backup = dir.join(format!("{filename}.bak-{}", now_seconds()));
        let _ = std::fs::copy(&path, &backup);

        for server in legacy.servers {
            let Some(url) = normalize_http_url(&server.url) else {
                notes.push(format!("{filename}: 跳过无效地址 {}", server.url));
                continue;
            };
            let key = url_key(&url);
            if seen_urls.iter().any(|seen| seen == &key) {
                duplicates += 1;
                continue;
            }
            seen_urls.push(key);
            let mut id = server.id.trim().to_string();
            if id.is_empty() || id == "local" || seen_ids.iter().any(|seen| seen == &id) {
                id = format!("{:016x}", rand::random::<u64>());
            }
            seen_ids.push(id.clone());
            config.targets.push(TargetEntry::Http(HttpTarget {
                id,
                name: server.name.trim().to_string(),
                url,
                allow_invalid_certs: server.allow_invalid_certs,
            }));
            if is_primary {
                local_count += 1;
            } else {
                client_added += 1;
            }
        }
        if !is_primary && legacy.active.is_some() {
            // The merged app always starts on local files; the old "active"
            // value is intentionally not carried over.
        }
    }

    if !seen_urls.is_empty() || config.targets.is_empty() {
        notes.push(format!(
            "配置迁移完成: local={local_count}, client_added={client_added}, duplicates={duplicates}"
        ));
    }
    config.active = Some("local".to_string());
    config
}

/// Same rules the old `normalize_server` applied to HTTP targets.
pub fn normalize_http_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let mut parsed = reqwest::Url::parse(&candidate).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if parsed.host_str().is_none() {
        return None;
    }
    parsed.set_fragment(None);
    parsed.set_query(None);
    let mut text = parsed.to_string();
    while text.ends_with('/') && text.len() > "https://x".len() {
        text.pop();
    }
    Some(text)
}

fn url_key(url: &str) -> String {
    url.to_ascii_lowercase()
}

pub fn save(dir: &Path, config: &TargetsConfig) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = config_path(dir);
    let raw = serde_json::to_vec_pretty(config)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw)?;
    std::fs::rename(&tmp, &path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn migrates_both_legacy_files_and_dedupes_urls() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            LEGACY_LOCAL,
            r#"{"active":"a","servers":[
                {"id":"a","name":"NAS","url":"https://192.168.1.10:8090/"},
                {"id":"b","name":"Home","url":"http://127.0.0.1:8090"}]}"#,
        );
        write(
            dir.path(),
            LEGACY_CLIENT,
            r#"{"active":"c","servers":[
                {"id":"c","name":"NAS copy","url":"https://192.168.1.10:8090"},
                {"id":"d","name":"Office","url":"office.example.com"}]}"#,
        );
        let (config, notes) = load_or_migrate(dir.path());
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.active.as_deref(), Some("local"));
        assert_eq!(config.targets.len(), 3);
        let urls: Vec<String> = config
            .targets
            .iter()
            .map(|target| match target {
                TargetEntry::Http(http) => http.url.clone(),
                TargetEntry::Ssh(_) => unreachable!(),
            })
            .collect();
        assert!(urls.contains(&"https://192.168.1.10:8090".to_string()));
        assert!(urls.contains(&"https://office.example.com".to_string()));
        assert!(notes.iter().any(|note| note.contains("duplicates=1")));
        // Legacy files stay in place and get a backup copy.
        assert!(dir.path().join(LEGACY_LOCAL).exists());
        assert!(dir
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".bak-")));
        // A second run reads targets.json and does not migrate again.
        let (again, _) = load_or_migrate(dir.path());
        assert_eq!(again.targets.len(), 3);
    }

    #[test]
    fn corrupt_targets_file_is_backed_up_and_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), CONFIG_FILENAME, "{not json");
        write(
            dir.path(),
            LEGACY_LOCAL,
            r#"{"servers":[{"id":"a","name":"NAS","url":"https://nas.example"}]}"#,
        );
        let (config, notes) = load_or_migrate(dir.path());
        assert_eq!(config.targets.len(), 1);
        assert!(notes.iter().any(|note| note.contains("已备份")));
        assert!(dir
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt-")));
    }

    #[test]
    fn empty_install_writes_a_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let (config, _) = load_or_migrate(dir.path());
        assert!(config.targets.is_empty());
        let raw = std::fs::read_to_string(config_path(dir.path())).unwrap();
        assert!(raw.contains("\"version\": 2"));
    }

    #[test]
    fn rejects_unsupported_schemes() {
        assert!(normalize_http_url("ftp://example.com").is_none());
        assert!(normalize_http_url("   ").is_none());
        assert_eq!(
            normalize_http_url("example.com:8090").as_deref(),
            Some("https://example.com:8090")
        );
    }

    #[test]
    fn normalize_active_falls_back_to_local() {
        let mut config = TargetsConfig::default();
        config.active = Some("gone".to_string());
        config.normalize_active(true);
        assert_eq!(config.active.as_deref(), Some("local"));
    }
}
