//! Trust-on-first-use store for SSH host keys.
//!
//! Pins are keyed by `host:port` and shared by every target pointing at the
//! same server. Deleting a target never deletes its pin; only an explicit
//! "forget" does.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

pub const KNOWN_HOSTS_FILENAME: &str = "known_hosts.json";
const VERSION: u32 = 1;

pub fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnownHostEntry {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// `key` for a plain host key, `certificate` for a host certificate.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Base64 of the SSH wire-format public key blob.
    pub key: String,
    /// `SHA256:<base64>` as printed by `ssh-keygen -lf`.
    pub fingerprint: String,
    #[serde(default)]
    pub added_at: u64,
    #[serde(default)]
    pub last_seen: u64,
}

fn default_kind() -> String {
    "key".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnownHostsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<KnownHostEntry>,
}

fn default_version() -> u32 {
    VERSION
}

impl Default for KnownHostsFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: Vec::new(),
        }
    }
}

/// Result of comparing a presented key with the stored pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyStatus {
    Trusted,
    Unknown(KnownHostEntry),
    Mismatch(KnownHostEntry),
}

pub fn host_key(
    host: &str,
    port: u16,
    algorithm: &str,
    key: &str,
    fingerprint: &str,
) -> KnownHostEntry {
    let now = now_seconds();
    KnownHostEntry {
        host: host.to_string(),
        port,
        algorithm: algorithm.to_string(),
        kind: "key".to_string(),
        key: key.to_string(),
        fingerprint: fingerprint.to_string(),
        added_at: now,
        last_seen: now,
    }
}

pub struct KnownHosts {
    path: PathBuf,
    entries: Vec<KnownHostEntry>,
}

impl KnownHosts {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(KNOWN_HOSTS_FILENAME);
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<KnownHostsFile>(&raw).ok())
            .map(|file| file.entries)
            .unwrap_or_default();
        Self { path, entries }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[KnownHostEntry] {
        &self.entries
    }

    pub fn verify(&self, entry: &KnownHostEntry) -> HostKeyStatus {
        match self
            .entries
            .iter()
            .find(|stored| stored.host == entry.host && stored.port == entry.port)
        {
            // Compare the wire blob, not just the fingerprint: two encodings
            // of the same key must still be byte-identical to be trusted.
            Some(stored) if stored.key == entry.key => HostKeyStatus::Trusted,
            Some(stored) => HostKeyStatus::Mismatch(stored.clone()),
            None => HostKeyStatus::Unknown(entry.clone()),
        }
    }

    /// Records a pin, replacing any previous key for the same host:port.
    pub fn trust(&mut self, entry: KnownHostEntry) -> std::io::Result<()> {
        self.entries
            .retain(|stored| !(stored.host == entry.host && stored.port == entry.port));
        self.entries.push(entry);
        self.save()
    }

    pub fn touch(&mut self, host: &str, port: u16) {
        if let Some(stored) = self
            .entries
            .iter_mut()
            .find(|stored| stored.host == host && stored.port == port)
        {
            stored.last_seen = now_seconds();
            let _ = self.save();
        }
    }

    /// Returns the number of removed pins.
    pub fn forget(&mut self, host: &str, port: u16) -> std::io::Result<usize> {
        let before = self.entries.len();
        self.entries
            .retain(|stored| !(stored.host == host && stored.port == port));
        let removed = before - self.entries.len();
        if removed > 0 {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = KnownHostsFile {
            version: VERSION,
            entries: self.entries.clone(),
        };
        let raw = serde_json::to_vec_pretty(&file)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(host: &str, key: &str) -> KnownHostEntry {
        host_key(host, 22, "ssh-ed25519", key, "SHA256:test")
    }

    #[test]
    fn unknown_then_trusted_then_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = KnownHosts::load(dir.path());
        assert!(matches!(
            store.verify(&entry("1.2.3.4", "AAAA")),
            HostKeyStatus::Unknown(_)
        ));
        store.trust(entry("1.2.3.4", "AAAA")).unwrap();
        assert_eq!(
            store.verify(&entry("1.2.3.4", "AAAA")),
            HostKeyStatus::Trusted
        );
        assert!(matches!(
            store.verify(&entry("1.2.3.4", "BBBB")),
            HostKeyStatus::Mismatch(_)
        ));
        // Pins are per host:port.
        assert!(matches!(
            store.verify(&entry("5.6.7.8", "AAAA")),
            HostKeyStatus::Unknown(_)
        ));
    }

    #[test]
    fn pins_survive_reload_and_can_be_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut store = KnownHosts::load(dir.path());
            store.trust(entry("nas.local", "AAAA")).unwrap();
        }
        let mut store = KnownHosts::load(dir.path());
        assert_eq!(store.entries().len(), 1);
        assert_eq!(store.forget("nas.local", 22).unwrap(), 1);
        assert_eq!(KnownHosts::load(dir.path()).entries().len(), 0);
        assert_eq!(store.forget("nas.local", 22).unwrap(), 0);
    }

    #[test]
    fn non_default_port_is_a_separate_pin() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = KnownHosts::load(dir.path());
        store
            .trust(host_key("host", 2222, "ssh-ed25519", "AAAA", "SHA256:x"))
            .unwrap();
        assert!(matches!(
            store.verify(&entry("host", "AAAA")),
            HostKeyStatus::Unknown(_)
        ));
    }
}
