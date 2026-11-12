//! Desktop update notifications and the in-app upgrade channel.
//!
//! The loopback companion performs every network request so the webview stays
//! under the strict `connect-src 'self'` CSP. GitHub's release API is public and
//! unauthenticated, and all checks are best effort: an offline machine must be
//! able to use the app exactly as before.
//!
//! Two independent channels are exposed to the UI:
//!
//! * *release channel* - always available. It compares the bundled version with
//!   the latest GitHub Release and picks the installer matching this machine's
//!   OS, architecture and app flavour.
//! * *silent channel* - only available when the release carries a signed
//!   updater manifest (`latest-local.json` / `latest-client.json`). The Tauri
//!   updater plugin then downloads, verifies and installs the update in place.

use std::{
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppMode;

pub const REPOSITORY: &str = "fengqshanl/rs-file-broswer";
pub const CURRENT_VERSION: &str = agentgod_files::VERSION;
const RELEASE_API: &str = "https://api.github.com/repos/fengqshanl/rs-file-broswer/releases/latest";
const RELEASE_PAGE: &str = "https://github.com/fengqshanl/rs-file-broswer/releases/latest";
const DOWNLOAD_PREFIX: &str =
    "https://github.com/fengqshanl/rs-file-broswer/releases/latest/download/";
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

impl AppMode {
    /// Slug used for release asset prefixes and updater manifest filenames.
    pub fn package_slug(self) -> &'static str {
        match self {
            AppMode::Local => "local",
            AppMode::Client => "client",
        }
    }

    fn asset_prefix(self) -> &'static str {
        match self {
            AppMode::Local => "agentgod.files.local_",
            AppMode::Client => "agentgod.files.client_",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct InstallStatus {
    /// idle | checking | downloading | installing | done | error
    pub phase: String,
    pub percent: u8,
    pub message: String,
}

impl InstallStatus {
    pub fn new(phase: &str, percent: u8, message: impl Into<String>) -> Self {
        Self {
            phase: phase.to_string(),
            percent,
            message: message.into(),
        }
    }

    pub fn idle() -> Self {
        Self::new("idle", 0, "尚未开始更新")
    }
}

/// Implemented by the Tauri shells. Keeping this as a trait means the shared
/// companion crate does not need to depend on `tauri` itself.
pub trait AutoInstaller: Send + Sync {
    fn available(&self) -> bool {
        false
    }
    fn start(&self) -> Result<(), String> {
        Err("此构建未启用静默更新".to_string())
    }
    fn restart(&self) -> Result<(), String> {
        Err("此构建不支持应用内重启".to_string())
    }
    fn status(&self) -> InstallStatus {
        InstallStatus::idle()
    }
}

pub struct NoopInstaller;

impl AutoInstaller for NoopInstaller {}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub app: String,
    pub platform: String,
    pub arch: String,
    pub asset: Option<String>,
    pub download_url: Option<String>,
    pub release_url: String,
    pub notes: String,
    /// A signed updater package exists for this exact platform.
    pub silent: bool,
    pub checked_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UpdateCheck {
    fn failed(app: AppMode, message: impl Into<String>) -> Self {
        Self {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: String::new(),
            has_update: false,
            app: app.package_slug().to_string(),
            platform: platform().0.to_string(),
            arch: platform().1.to_string(),
            asset: None,
            download_url: None,
            release_url: RELEASE_PAGE.to_string(),
            notes: String::new(),
            silent: false,
            checked_at: now_seconds(),
            error: Some(message.into()),
        }
    }
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Clone, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

pub struct UpdateHub {
    app: AppMode,
    client: reqwest::Client,
    installer: Arc<dyn AutoInstaller>,
    cached: Mutex<Option<(Instant, UpdateCheck)>>,
}

impl UpdateHub {
    pub fn new(app: AppMode, installer: Arc<dyn AutoInstaller>) -> Arc<Self> {
        Arc::new(Self {
            app,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            installer,
            cached: Mutex::new(None),
        })
    }

    pub async fn check(&self, force: bool) -> UpdateCheck {
        if !force {
            if let Ok(guard) = self.cached.lock() {
                if let Some((at, ref value)) = *guard {
                    if at.elapsed() < CACHE_TTL {
                        return value.clone();
                    }
                }
            }
        }
        let result = match self.fetch_check().await {
            Ok(value) => value,
            Err(err) => UpdateCheck::failed(self.app, format!("检查更新失败：{err}")),
        };
        if let Ok(mut guard) = self.cached.lock() {
            *guard = Some((Instant::now(), result.clone()));
        }
        result
    }

    async fn fetch_check(&self) -> anyhow::Result<UpdateCheck> {
        let release: GitHubRelease = self
            .client
            .get(RELEASE_API)
            .header("User-Agent", format!("AgentGodFiles/{}", CURRENT_VERSION))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let latest = normalize_version(&release.tag_name)
            .ok_or_else(|| anyhow::anyhow!("无法解析发布版本 {}", release.tag_name))?;
        let (os, arch) = platform();
        let has_update = is_newer(&latest, CURRENT_VERSION);
        let asset = pick_asset(&release.assets, self.app, os, arch);
        let silent = has_update && self.silent_available(os, arch).await;

        Ok(UpdateCheck {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: latest,
            has_update,
            app: self.app.package_slug().to_string(),
            platform: os.to_string(),
            arch: arch.to_string(),
            download_url: asset
                .as_ref()
                .map(|value| value.browser_download_url.clone()),
            asset: asset.map(|value| value.name),
            release_url: RELEASE_PAGE.to_string(),
            notes: truncate(release.body.as_deref().unwrap_or("").trim(), 600),
            silent,
            checked_at: now_seconds(),
            error: None,
        })
    }

    /// A signed static manifest must exist and cover this machine's target,
    /// otherwise the one-click updater would fail after the user clicks it.
    async fn silent_available(&self, os: &str, arch: &str) -> bool {
        if !self.installer.available() {
            return false;
        }
        let url = format!("{}latest-{}.json", DOWNLOAD_PREFIX, self.app.package_slug());
        let Ok(response) = self.client.get(url).send().await else {
            return false;
        };
        if !response.status().is_success() {
            return false;
        }
        let Ok(manifest) = response.json::<Value>().await else {
            return false;
        };
        let targets = updater_targets(os, arch);
        targets.iter().any(|target| {
            manifest
                .get("platforms")
                .and_then(|platforms| platforms.get(target))
                .and_then(|entry| entry.get("url"))
                .and_then(Value::as_str)
                .is_some_and(|url| url.starts_with("https://"))
        })
    }

    /// Opens the installer that the last successful check selected. The URL is
    /// never accepted from the webview, so this cannot be used as an SSRF or
    /// arbitrary command launcher.
    pub fn open_download(&self) -> Result<(), String> {
        let url = self
            .cached
            .lock()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .and_then(|(_, check)| check.download_url.clone())
            })
            // No platform-matched installer: fall back to the release page.
            .unwrap_or_else(|| RELEASE_PAGE.to_string());
        if !url.starts_with("https://github.com/fengqshanl/rs-file-broswer/releases/") {
            return Err("下载地址无效".to_string());
        }
        open_external(&url)
    }

    pub fn start_install(&self) -> Result<(), String> {
        if !self.installer.available() {
            return Err("此构建未启用静默更新，请下载安装包手动升级".to_string());
        }
        self.installer.start()
    }

    pub fn restart(&self) -> Result<(), String> {
        self.installer.restart()
    }

    pub fn install_status(&self) -> InstallStatus {
        self.installer.status()
    }
}

fn open_external(url: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("无法打开下载链接：{err}"))
}

/// OS and architecture as reported by the running binary.
pub fn platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        _ => "unknown",
    };
    (os, std::env::consts::ARCH)
}

/// Tauri updater uses `darwin-x86_64` style keys; a universal macOS package
/// covers both Apple architectures.
fn updater_targets(os: &str, arch: &str) -> Vec<String> {
    if os == "macos" {
        return vec![format!("darwin-{arch}"), "darwin-universal".to_string()];
    }
    if os == "unknown" {
        return Vec::new();
    }
    vec![format!("{os}-{arch}")]
}

/// `desktop-v0.1.3` / `v0.1.3` / `0.1.3` -> `0.1.3`.
pub fn normalize_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("desktop-v")
        .or_else(|| trimmed.strip_prefix("desktop-V"))
        .or_else(|| trimmed.strip_prefix('v'))
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let core = stripped.split(['-', '+']).next().unwrap_or(stripped);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.parse::<u64>().is_err()) {
        return None;
    }
    Some(core.to_string())
}

/// Strict semver-ish comparison of `major.minor.patch`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(latest), Some(current)) = (version_parts(latest), version_parts(current)) else {
        return false;
    };
    latest > current
}

fn version_parts(version: &str) -> Option<(u64, u64, u64)> {
    let normalized = normalize_version(version)?;
    let mut parts = normalized.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

const IGNORED_SUFFIXES: &[&str] = &[".sig", ".json", ".tar.gz", ".zip", ".txt", ".yml"];

fn pick_asset(assets: &[ReleaseAsset], app: AppMode, os: &str, arch: &str) -> Option<ReleaseAsset> {
    let candidates: Vec<&ReleaseAsset> = assets
        .iter()
        .filter(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.starts_with(app.asset_prefix())
                && !IGNORED_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
                && asset.browser_download_url.starts_with("https://")
        })
        .collect();

    // Ordered by preference: an installer the OS can run directly first.
    let extensions: &[&str] = match os {
        "macos" => &[".dmg"],
        "windows" => &["-setup.exe", ".msi"],
        "linux" => &[".appimage", ".deb"],
        _ => &[],
    };
    for extension in extensions {
        for asset in &candidates {
            let name = asset.name.to_ascii_lowercase();
            if name.ends_with(extension) && arch_matches(&name, arch, os) {
                return Some((*asset).clone());
            }
        }
    }
    None
}

fn arch_matches(name: &str, arch: &str, os: &str) -> bool {
    if name.contains("universal") {
        return true;
    }
    match arch {
        "aarch64" => {
            name.contains("aarch64")
                || name.contains("arm64")
                // Windows on ARM runs x64 installers through emulation.
                || (os == "windows" && name.contains("x64"))
        }
        "x86_64" => name.contains("x64") || name.contains("amd64") || name.contains("x86_64"),
        _ => false,
    }
}

fn truncate(text: &str, limit: usize) -> String {
    let mut out: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        out.push('…');
    }
    out
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            browser_download_url: format!("https://github.com/x/y/releases/download/v1/{name}"),
        }
    }

    fn release_assets() -> Vec<ReleaseAsset> {
        vec![
            asset("AgentGod.Files.Local_0.1.2_amd64.AppImage"),
            asset("AgentGod.Files.Local_0.1.2_amd64.deb"),
            asset("AgentGod.Files.Local_0.1.2_universal.dmg"),
            asset("AgentGod.Files.Local_0.1.2_x64-setup.exe"),
            asset("AgentGod.Files.Local_0.1.2_x64_en-US.msi"),
            asset("AgentGod.Files.Local_0.1.2_x64-setup.exe.sig"),
            asset("latest-local.json"),
            asset("AgentGod.Files.Client_0.1.2_x64-setup.exe"),
        ]
    }

    #[test]
    fn normalizes_release_tags() {
        assert_eq!(
            normalize_version("desktop-v0.1.3").as_deref(),
            Some("0.1.3")
        );
        assert_eq!(normalize_version("v0.1.3").as_deref(), Some("0.1.3"));
        assert_eq!(normalize_version("0.2.0-beta.1").as_deref(), Some("0.2.0"));
        assert_eq!(normalize_version("nightly"), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.2", "0.1.2"));
        assert!(!is_newer("0.1.1", "0.1.2"));
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn picks_the_installer_for_this_machine() {
        let assets = release_assets();
        assert_eq!(
            pick_asset(&assets, AppMode::Local, "windows", "x86_64")
                .unwrap()
                .name,
            "AgentGod.Files.Local_0.1.2_x64-setup.exe"
        );
        assert_eq!(
            pick_asset(&assets, AppMode::Local, "macos", "aarch64")
                .unwrap()
                .name,
            "AgentGod.Files.Local_0.1.2_universal.dmg"
        );
        assert_eq!(
            pick_asset(&assets, AppMode::Local, "linux", "x86_64")
                .unwrap()
                .name,
            "AgentGod.Files.Local_0.1.2_amd64.AppImage"
        );
        assert_eq!(
            pick_asset(&assets, AppMode::Client, "windows", "x86_64")
                .unwrap()
                .name,
            "AgentGod.Files.Client_0.1.2_x64-setup.exe"
        );
    }

    #[test]
    fn never_suggests_a_signature_or_manifest() {
        let assets = release_assets();
        assert!(pick_asset(&assets, AppMode::Local, "windows", "armv7").is_none());
        assert!(pick_asset(&assets, AppMode::Client, "linux", "x86_64").is_none());
    }

    #[test]
    fn updater_targets_cover_universal_macos() {
        assert_eq!(
            updater_targets("macos", "aarch64"),
            vec!["darwin-aarch64", "darwin-universal"]
        );
        assert_eq!(updater_targets("linux", "x86_64"), vec!["linux-x86_64"]);
    }
}
