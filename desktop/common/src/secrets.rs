//! Credential storage for SSH targets.
//!
//! Only passwords and private-key passphrases are stored, and only in the OS
//! credential store (Windows Credential Manager / macOS Keychain). Private key
//! material never leaves the user's file.

use std::collections::HashMap;

use anyhow::{Context, Result};

const SERVICE: &str = "com.agentgod.files";

pub trait Secrets: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn set(&self, account: &str, value: &str) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}

pub fn account(id: &str, field: &str) -> String {
    format!("ssh:{id}:{field}")
}

pub struct KeyringSecrets {
    service: String,
}

impl Default for KeyringSecrets {
    fn default() -> Self {
        Self {
            service: SERVICE.to_string(),
        }
    }
}

impl KeyringSecrets {
    fn entry(&self, account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, account).context("无法访问系统凭据管理器")
    }
}

impl Secrets for KeyringSecrets {
    fn get(&self, account: &str) -> Result<Option<String>> {
        match self.entry(account)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(anyhow::Error::new(err)).context("读取系统凭据失败"),
        }
    }

    fn set(&self, account: &str, value: &str) -> Result<()> {
        self.entry(account)?
            .set_password(value)
            .context("写入系统凭据失败")
    }

    fn delete(&self, account: &str) -> Result<()> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(anyhow::Error::new(err)).context("删除系统凭据失败"),
        }
    }
}

/// In-memory store for tests and for headless development runs.
#[derive(Default)]
pub struct MemorySecrets {
    values: std::sync::Mutex<HashMap<String, String>>,
}

impl Secrets for MemorySecrets {
    fn get(&self, account: &str) -> Result<Option<String>> {
        Ok(self
            .values
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(account)
            .cloned())
    }

    fn set(&self, account: &str, value: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .insert(account.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let store = MemorySecrets::default();
        let key = account("abc", "password");
        assert_eq!(store.get(&key).unwrap(), None);
        store.set(&key, "secret").unwrap();
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("secret"));
        store.delete(&key).unwrap();
        assert_eq!(store.get(&key).unwrap(), None);
    }

    #[test]
    fn account_names_are_scoped_per_target() {
        assert_eq!(account("t1", "password"), "ssh:t1:password");
        assert_ne!(account("t1", "password"), account("t2", "password"));
    }
}
