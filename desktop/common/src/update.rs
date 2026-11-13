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
//!   updater manifest (`latest.json`). The Tauri updater plugin then downloads,
//!   verifies and installs the update in place.

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const REPOSITORY: &str = "fengqshanl/rs-file-broswer";
pub const CURRENT_VERSION: &str = agentgod_files::VERSION;
pub const DEFAULT_API_URL: &str =
    "https://api.github.com/repos/fengqshanl/rs-file-broswer/releases/latest";
pub const DEFAULT_RELEASE_PAGE: &str =
    "https://github.com/fengqshanl/rs-file-broswer/releases/latest";
pub const DEFAULT_DOWNLOAD_PREFIX: &str =
    "https://github.com/fengqshanl/rs-file-broswer/releases/latest/download/";
const ENV_API_URL: &str = "AGENTGOD_UPDATE_API";
const ENV_DOWNLOAD_PREFIX: &str = "AGENTGOD_UPDATE_DOWNLOAD_PREFIX";
const ENV_RELEASE_PAGE: &str = "AGENTGOD_UPDATE_RELEASE_PAGE";
const ENV_ENDPOINTS: &str = "AGENTGOD_UPDATER_ENDPOINTS";
const UPDATE_SOURCE_FILE: &str = "update-source.json";
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

fn default_api_url() -> String {
    DEFAULT_API_URL.to_string()
}

fn default_download_prefix() -> String {
    DEFAULT_DOWNLOAD_PREFIX.to_string()
}

fn default_release_page() -> String {
    DEFAULT_RELEASE_PAGE.to_string()
}

fn https_url(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("http://localhost")
}

/// Where the desktop apps look for releases and updater manifests.
///
/// GitHub is not reachable from every network (mainland China in particular),
/// so both the release check and the signed updater channel can be pointed at
/// a mirror. Values are read from `<app data>/update-source.json` and can be
/// overridden with environment variables. Every field keeps its GitHub default
/// when omitted, so an absent file changes nothing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateSource {
    #[serde(default = "default_api_url")]
    pub api_url: String,
    #[serde(default = "default_download_prefix")]
    pub download_prefix: String,
    #[serde(default = "default_release_page")]
    pub release_page: String,
    /// Explicit Tauri updater manifest URLs. Empty means derive them from
    /// `download_prefix` as `latest-<app>.json`.
    #[serde(default)]
    pub updater_endpoints: Vec<String>,
}

impl Default for UpdateSource {
    fn default() -> Self {
        Self {
            api_url: DEFAULT_API_URL.to_string(),
            download_prefix: DEFAULT_DOWNLOAD_PREFIX.to_string(),
            release_page: DEFAULT_RELEASE_PAGE.to_string(),
            updater_endpoints: Vec::new(),
        }
    }
}

impl UpdateSource {
    pub fn config_path(data_dir: &Path) -> PathBuf {
        data_dir.join(UPDATE_SOURCE_FILE)
    }

    /// Reads the optional config file, then applies environment overrides.
    /// Invalid values fall back to the defaults instead of blocking startup.
    pub fn load(data_dir: &Path) -> Self {
        let path = Self::config_path(data_dir);
        let mut source = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<UpdateSource>(&raw).unwrap_or_else(|err| {
                agentgod_files::error::log(&format!(
                    "更新源配置解析失败，使用默认 GitHub 源: {} ({err})",
                    path.display()
                ));
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        if let Some(value) = env_value(ENV_API_URL) {
            source.api_url = value;
        }
        if let Some(value) = env_value(ENV_DOWNLOAD_PREFIX) {
            source.download_prefix = value;
        }
        if let Some(value) = env_value(ENV_RELEASE_PAGE) {
            source.release_page = value;
        }
        if let Some(value) = env_value(ENV_ENDPOINTS) {
            source.updater_endpoints = value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect();
        }

        source.normalize();
        source
    }

    /// Rejects plaintext or non-HTTP override values and normalizes the
    /// download prefix so `manifest_url` never emits a doubled slash.
    fn normalize(&mut self) {
        let fallback = Self::default();
        if !https_url(&self.api_url) {
            agentgod_files::error::log("更新源 api_url 必须是 https 地址，已回退到 GitHub");
            self.api_url = fallback.api_url;
        }
        if !https_url(&self.download_prefix) {
            agentgod_files::error::log("更新源 download_prefix 必须是 https 地址，已回退到 GitHub");
            self.download_prefix = fallback.download_prefix;
        }
        if !https_url(&self.release_page) {
            self.release_page = fallback.release_page;
        }
        self.updater_endpoints.retain(|url| https_url(url));
        self.download_prefix = format!("{}/", self.download_prefix.trim_end_matches('/'));
    }

    pub fn manifest_url(&self) -> String {
        format!("{}latest.json", self.download_prefix)
    }

    pub fn endpoints(&self) -> Vec<String> {
        if self.updater_endpoints.is_empty() {
            vec![self.manifest_url()]
        } else {
            self.updater_endpoints.clone()
        }
    }

    /// Short, actionable hint shown when the release check fails.
    pub fn hint(&self, data_dir: &Path) -> String {
        if self.api_url == DEFAULT_API_URL {
            format!(
                "如无法访问 GitHub，可在 {} 中配置镜像源（api_url / download_prefix）",
                Self::config_path(data_dir).display()
            )
        } else {
            String::new()
        }
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Platforms with published installers. Only Windows MSI packages are built
/// today; the update check must not offer a download that is not released.
/// Add an entry here (plus a manifest rule) to re-enable a platform.
pub const SUPPORTED_PLATFORMS: &[&str] = &["windows"];

/// Prefix of the published Windows installer assets.
const ASSET_PREFIX: &str = "agentgod.files_";

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
    fn failed(release_url: String, message: impl Into<String>) -> Self {
        Self {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: String::new(),
            has_update: false,
            app: "app".to_string(),
            platform: platform().0.to_string(),
            arch: platform().1.to_string(),
            asset: None,
            download_url: None,
            release_url,
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
    client: reqwest::Client,
    installer: Arc<dyn AutoInstaller>,
    source: UpdateSource,
    data_dir: PathBuf,
    cached: Mutex<Option<(Instant, UpdateCheck)>>,
}

impl UpdateHub {
    pub fn new(
        installer: Arc<dyn AutoInstaller>,
        source: UpdateSource,
        data_dir: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .connect_timeout(Duration::from_secs(8))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            installer,
            source,
            data_dir,
            cached: Mutex::new(None),
        })
    }

    /// The source is also consumed by the Tauri updater shell, so expose it.
    pub fn source(&self) -> &UpdateSource {
        &self.source
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
            Err(err) => {
                let hint = self.source.hint(&self.data_dir);
                let message = if hint.is_empty() {
                    format!("检查更新失败：{err}")
                } else {
                    format!("检查更新失败：{err}。{hint}")
                };
                UpdateCheck::failed(self.source.release_page.clone(), message)
            }
        };
        if let Ok(mut guard) = self.cached.lock() {
            *guard = Some((Instant::now(), result.clone()));
        }
        result
    }

    async fn fetch_check(&self) -> anyhow::Result<UpdateCheck> {
        let release = self.fetch_release().await?;

        let latest = normalize_version(&release.tag_name)
            .ok_or_else(|| anyhow::anyhow!("无法解析发布版本 {}", release.tag_name))?;
        let (os, arch) = platform();
        let has_update = is_newer(&latest, CURRENT_VERSION);
        let asset = pick_asset(&release.assets, os, arch);
        let silent = has_update && self.silent_available(os, arch).await;

        Ok(UpdateCheck {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: latest,
            has_update,
            app: "app".to_string(),
            platform: os.to_string(),
            arch: arch.to_string(),
            download_url: asset
                .as_ref()
                .map(|value| value.browser_download_url.clone()),
            asset: asset.map(|value| value.name),
            release_url: self.source.release_page.clone(),
            notes: truncate(release.body.as_deref().unwrap_or("").trim(), 600),
            silent,
            checked_at: now_seconds(),
            error: None,
        })
    }

    /// One retry covers transient DNS/TLS failures on flaky networks without
    /// making a manual "check now" wait a minute.
    async fn fetch_release(&self) -> anyhow::Result<GitHubRelease> {
        let mut last_error = None;
        for attempt in 0..2 {
            let result = self
                .client
                .get(&self.source.api_url)
                .header("User-Agent", format!("AgentGodFiles/{}", CURRENT_VERSION))
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .and_then(|response| response.error_for_status())
                .map_err(anyhow::Error::from);
            match result {
                Ok(response) => return Ok(response.json().await?),
                Err(err) => {
                    last_error = Some(err);
                    if attempt == 0 {
                        tokio::time::sleep(Duration::from_millis(600)).await;
                    }
                }
            }
        }
        Err(last_error.expect("retry loop always assigns an error"))
    }

    /// A signed static manifest must exist and cover this machine's target,
    /// otherwise the one-click updater would fail after the user clicks it.
    async fn silent_available(&self, os: &str, arch: &str) -> bool {
        if !self.installer.available() {
            return false;
        }
        let url = self.source.manifest_url();
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
            .unwrap_or_else(|| self.source.release_page.clone());
        if !https_url(&url) {
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

/// Tauri updater target keys. The published MSI is x64, which also covers
/// Windows on ARM because the OS runs x64 binaries through emulation, so a
/// `windows-x86_64` entry is a valid update for both.
fn updater_targets(os: &str, arch: &str) -> Vec<String> {
    if !SUPPORTED_PLATFORMS.contains(&os) {
        return Vec::new();
    }
    match os {
        "windows" => {
            if arch == "aarch64" {
                vec!["windows-aarch64".to_string(), "windows-x86_64".to_string()]
            } else {
                vec![format!("windows-{arch}")]
            }
        }
        "macos" => vec![format!("darwin-{arch}"), "darwin-universal".to_string()],
        _ => vec![format!("{os}-{arch}")],
    }
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

fn pick_asset(assets: &[ReleaseAsset], os: &str, arch: &str) -> Option<ReleaseAsset> {
    if !SUPPORTED_PLATFORMS.contains(&os) {
        return None;
    }
    let candidates: Vec<&ReleaseAsset> = assets
        .iter()
        .filter(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.starts_with(ASSET_PREFIX)
                && !IGNORED_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
                && asset.browser_download_url.starts_with("https://")
        })
        .collect();

    // Ordered by preference. Windows uses the MSI package because that is the
    // only desktop installer the release workflow publishes.
    let extensions: &[&str] = match os {
        "windows" => &[".msi"],
        "macos" => &[".dmg"],
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
            asset("AgentGod.Files_0.2.0_x64.msi"),
            asset("AgentGod.Files_0.2.0_x64.msi.sig"),
            // Assets from the retired macOS / Linux / NSIS channels, and the
            // pre-merge per-app builds, must be ignored.
            asset("AgentGod.Files_0.2.0_universal.dmg"),
            asset("AgentGod.Files_0.2.0_universal.app.tar.gz"),
            asset("AgentGod.Files_0.2.0_amd64.AppImage"),
            asset("AgentGod.Files_0.2.0_amd64.deb"),
            asset("AgentGod.Files_0.2.0_x64-setup.exe"),
            asset("AgentGod.Files.Local_0.1.6_x64_en-US.msi"),
            asset("AgentGod.Files.Client_0.1.6_x64_en-US.msi"),
            asset("latest-local.json"),
            asset("latest.json"),
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
    fn picks_the_msi_for_windows() {
        let assets = release_assets();
        assert_eq!(
            pick_asset(&assets, "windows", "x86_64").unwrap().name,
            "AgentGod.Files_0.2.0_x64.msi"
        );
        // Windows on ARM runs the x64 MSI through emulation.
        assert_eq!(
            pick_asset(&assets, "windows", "aarch64").unwrap().name,
            "AgentGod.Files_0.2.0_x64.msi"
        );
    }

    #[test]
    fn macos_and_linux_get_no_installer() {
        let assets = release_assets();
        assert!(pick_asset(&assets, "macos", "aarch64").is_none());
        assert!(pick_asset(&assets, "macos", "x86_64").is_none());
        assert!(pick_asset(&assets, "linux", "x86_64").is_none());
        assert_eq!(updater_targets("macos", "aarch64"), Vec::<String>::new());
        assert_eq!(updater_targets("linux", "x86_64"), Vec::<String>::new());
    }

    #[test]
    fn updater_targets_cover_windows() {
        assert_eq!(updater_targets("windows", "x86_64"), vec!["windows-x86_64"]);
        assert_eq!(
            updater_targets("windows", "aarch64"),
            vec!["windows-aarch64", "windows-x86_64"]
        );
    }

    #[test]
    fn update_source_defaults_to_github() {
        let source = UpdateSource::default();
        assert_eq!(source.api_url, DEFAULT_API_URL);
        assert_eq!(
            source.manifest_url(),
            "https://github.com/fengqshanl/rs-file-broswer/releases/latest/download/latest.json"
        );
        assert_eq!(source.endpoints(), vec![source.manifest_url()]);
        assert!(source
            .hint(std::path::Path::new("."))
            .contains("update-source.json"));
    }

    #[test]
    fn update_source_accepts_a_mirror() {
        let mut source = UpdateSource {
            api_url: "https://mirror.example/releases/latest".to_string(),
            download_prefix: "https://mirror.example/releases/latest/download".to_string(),
            release_page: "https://mirror.example/releases".to_string(),
            updater_endpoints: vec!["https://mirror.example/updater/latest.json".to_string()],
        };
        source.normalize();
        assert_eq!(
            source.manifest_url(),
            "https://mirror.example/releases/latest/download/latest.json"
        );
        assert_eq!(
            source.endpoints(),
            vec!["https://mirror.example/updater/latest.json"]
        );
        assert!(source.hint(std::path::Path::new(".")).is_empty());
    }

    #[test]
    fn update_source_rejects_non_https_overrides() {
        let mut source = UpdateSource {
            api_url: "http://evil.example/api".to_string(),
            download_prefix: "file:///tmp/".to_string(),
            release_page: "http://evil.example/page".to_string(),
            updater_endpoints: vec!["http://evil.example/manifest.json".to_string()],
        };
        source.normalize();
        assert_eq!(source.api_url, DEFAULT_API_URL);
        assert_eq!(source.download_prefix, DEFAULT_DOWNLOAD_PREFIX);
        assert_eq!(source.release_page, DEFAULT_RELEASE_PAGE);
        assert!(source.updater_endpoints.is_empty());
    }
}
