//! One-time migration from the AgentGod Files profile to RockBadger.
//!
//! The product rename changed the data directory and the OS credential-store
//! service name. Both are migrated once, without deleting the old data: a user
//! who rolls back to 0.2.x must still find their settings and secrets.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    config::TargetEntry,
    known_hosts::now_seconds,
    secrets::{self, Secrets},
};

pub const MIGRATION_FILENAME: &str = "migration.json";
pub const DATA_DIR_NAME: &str = "RockBadger";
pub const LEGACY_DATA_DIR_NAME: &str = "AgentGodFiles";

/// Files that make up the user's workspace. Caches, temp uploads and generated
/// TLS material are deliberately excluded; they regenerate on first launch.
const COPIED_FILES: &[&str] = &[
    "targets.json",
    "known_hosts.json",
    "update-source.json",
    "settings.json",
    "servers-local.json",
    "servers-client.json",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MigrationMarker {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub data_from: Option<String>,
    #[serde(default)]
    pub data_migrated_at: Option<u64>,
    #[serde(default)]
    pub keyring_migrated_at: Option<u64>,
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join(MIGRATION_FILENAME)
}

pub fn load_marker(dir: &Path) -> MigrationMarker {
    fs::read(marker_path(dir))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save_marker(dir: &Path, marker: &MigrationMarker) -> std::io::Result<()> {
    let path = marker_path(dir);
    let raw = serde_json::to_vec_pretty(marker)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw)?;
    fs::rename(&tmp, &path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn legacy_data_dir() -> Option<PathBuf> {
    dirs::data_dir()
        .map(|dir| dir.join(LEGACY_DATA_DIR_NAME))
        .filter(|dir| dir.is_dir())
}

/// Copies the whitelisted profile files once. Returns how many files were
/// copied. Never overwrites files that already exist in `new_dir`, and writes
/// the marker only after every copy succeeded so a partial run retries.
pub fn migrate_data(old_dir: &Path, new_dir: &Path) -> std::io::Result<usize> {
    fs::create_dir_all(new_dir)?;
    let mut marker = load_marker(new_dir);
    if marker.data_migrated_at.is_some() {
        return Ok(0);
    }

    let mut copied = 0;
    for name in COPIED_FILES {
        let source = old_dir.join(name);
        if !source.is_file() {
            continue;
        }
        let destination = new_dir.join(name);
        if destination.exists() {
            continue;
        }
        fs::copy(&source, &destination)?;
        copied += 1;
    }

    marker.version = marker.version.max(1);
    marker.data_from = Some(old_dir.display().to_string());
    marker.data_migrated_at = Some(now_seconds());
    save_marker(new_dir, &marker)?;
    Ok(copied)
}

/// Resolves the profile directory and imports the legacy profile once.
///
/// An explicit `ROCKBADGER_DATA_DIR` / `AGENTGOD_DATA_DIR` means the caller
/// selected a self-contained profile (tests, portable installs), so nothing is
/// imported from the real user profile.
pub fn prepare_data_dir() -> std::io::Result<PathBuf> {
    let new_dir = crate::data_dir();
    let explicit = std::env::var_os("ROCKBADGER_DATA_DIR").is_some()
        || std::env::var_os("AGENTGOD_DATA_DIR").is_some();
    fs::create_dir_all(&new_dir)?;
    if explicit {
        return Ok(new_dir);
    }
    let Some(old_dir) = legacy_data_dir() else {
        return Ok(new_dir);
    };
    if old_dir == new_dir {
        return Ok(new_dir);
    }
    match migrate_data(&old_dir, &new_dir) {
        Ok(0) => {}
        Ok(copied) => rockbadger::error::log(&format!(
            "已从 {} 迁移 {copied} 个配置文件（旧目录保留）",
            old_dir.display()
        )),
        Err(err) => rockbadger::error::log(&format!(
            "迁移旧配置失败（下次启动重试）: {err}"
        )),
    }
    Ok(new_dir)
}

pub fn mark_keyring_migrated(dir: &Path) {
    let mut marker = load_marker(dir);
    marker.version = marker.version.max(1);
    marker.keyring_migrated_at = Some(now_seconds());
    if let Err(err) = save_marker(dir, &marker) {
        rockbadger::error::log(&format!("写入迁移标记失败: {err}"));
    }
}

/// Copies SSH passwords and key passphrases from the old credential-store
/// service to the new one. Existing new values win and old entries are kept.
pub fn migrate_keyring(
    old: &dyn Secrets,
    new: &dyn Secrets,
    targets: &[TargetEntry],
) -> Result<usize, String> {
    let mut copied = 0;
    for target in targets {
        let TargetEntry::Ssh(ssh) = target else {
            continue;
        };
        for field in ["password", "passphrase"] {
            let key = secrets::account(&ssh.id, field);
            match new.get(&key) {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(err) => return Err(format!("读取新凭据失败: {err}")),
            }
            match old.get(&key) {
                Ok(Some(value)) => {
                    new.set(&key, &value)
                        .map_err(|err| format!("写入新凭据失败: {err}"))?;
                    copied += 1;
                }
                Ok(None) => {}
                Err(err) => return Err(format!("读取旧凭据失败: {err}")),
            }
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{SshAuth, SshTarget},
        secrets::MemorySecrets,
    };

    #[test]
    fn copies_whitelisted_files_once_and_keeps_old_dir() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(LEGACY_DATA_DIR_NAME);
        let new = root.path().join(DATA_DIR_NAME);
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("targets.json"), "{\"version\":2,\"targets\":[]}").unwrap();
        fs::write(old.join("known_hosts.json"), "{}").unwrap();
        fs::write(old.join("tmp-local"), "ignored").unwrap();

        let copied = migrate_data(&old, &new).unwrap();
        assert_eq!(copied, 2);
        assert!(new.join("targets.json").is_file());
        assert!(new.join("known_hosts.json").is_file());
        assert!(!new.join("tmp-local").exists());
        assert!(old.join("targets.json").is_file(), "old profile must stay");

        // Existing destination files are never overwritten.
        fs::write(new.join("targets.json"), "{\"version\":2,\"targets\":[1]}").unwrap();
        fs::remove_file(new.join("known_hosts.json")).unwrap();
        fs::remove_file(marker_path(&new)).unwrap();
        let copied = migrate_data(&old, &new).unwrap();
        assert_eq!(copied, 1, "only the missing known_hosts is copied again");
        let kept = fs::read_to_string(new.join("targets.json")).unwrap();
        assert!(kept.contains("[1]"));

        // The marker stops a third run entirely.
        let copied = migrate_data(&old, &new).unwrap();
        assert_eq!(copied, 0);
        assert!(load_marker(&new).data_migrated_at.is_some());
    }

    #[test]
    fn keyring_migration_is_idempotent_and_never_overwrites() {
        let old = MemorySecrets::default();
        let new = MemorySecrets::default();
        let target = TargetEntry::Ssh(SshTarget {
            id: "abc".into(),
            name: "vps".into(),
            host: "127.0.0.1".into(),
            port: 22,
            username: "root".into(),
            auth: SshAuth::Password,
            key_path: None,
            root: None,
        });
        let targets = vec![target];
        let password = secrets::account("abc", "password");
        old.set(&password, "secret").unwrap();

        assert_eq!(migrate_keyring(&old, &new, &targets).unwrap(), 1);
        assert_eq!(new.get(&password).unwrap().as_deref(), Some("secret"));
        assert_eq!(old.get(&password).unwrap().as_deref(), Some("secret"));

        // Second run sees the value already present.
        assert_eq!(migrate_keyring(&old, &new, &targets).unwrap(), 0);

        // A value set in the new app wins over the old one.
        new.set(&password, "newer").unwrap();
        assert_eq!(migrate_keyring(&old, &new, &targets).unwrap(), 0);
        assert_eq!(new.get(&password).unwrap().as_deref(), Some("newer"));
    }

    #[test]
    fn keyring_migration_reports_store_errors() {
        struct Failing;
        impl Secrets for Failing {
            fn get(&self, _: &str) -> anyhow::Result<Option<String>> {
                Err(anyhow::anyhow!("locked"))
            }
            fn set(&self, _: &str, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn delete(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let target = TargetEntry::Ssh(SshTarget {
            id: "abc".into(),
            name: "vps".into(),
            host: "127.0.0.1".into(),
            port: 22,
            username: "root".into(),
            auth: SshAuth::Password,
            key_path: None,
            root: None,
        });
        let new = MemorySecrets::default();
        let err = migrate_keyring(&Failing, &new, &[target]).unwrap_err();
        assert!(err.contains("旧凭据"));
    }
}
