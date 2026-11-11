use std::{
    net::IpAddr,
    path::PathBuf,
    sync::{atomic::AtomicU64, Arc},
    time::{Duration, Instant},
};

use dashmap::DashMap;
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::{
    config::Config, credentials::Credentials, crypto::Crypto, preview::PreviewToken,
    rate::RateLimiter,
};

pub struct AppState {
    pub cfg: Arc<Config>,
    pub crypto: Crypto,
    pub credentials: RwLock<Credentials>,
    pub credentials_path: PathBuf,
    pub credentials_update: Mutex<()>,
    pub sessions: DashMap<String, Session>,
    pub preview_tokens: DashMap<String, PreviewToken>,
    pub login_guard: LoginGuard,
    pub uploads: DashMap<String, Arc<UploadSession>>,
    pub upload_limiter: Arc<RateLimiter>,
    pub download_limiter: Arc<RateLimiter>,
    pub upload_sem: Arc<Semaphore>,
    pub download_sem: Arc<Semaphore>,
    pub preview_sem: Arc<Semaphore>,
    pub login_sem: Arc<Semaphore>,
}

pub struct Session {
    pub key: [u8; 32],
    pub user: String,
    pub expires_at: Instant,
    pub hard_expires_at: Instant,
}

pub struct UploadSession {
    pub owner: String,
    pub inner: Mutex<UploadInner>,
    pub last_active: AtomicU64,
}

pub struct UploadInner {
    pub temp_path: PathBuf,
    pub final_path: PathBuf,
    pub logical_path: String,
    pub declared: u64,
    pub received: u64,
    pub next_seq: u64,
    pub file: Option<tokio::fs::File>,
}

pub struct LoginGuard {
    inner: DashMap<IpAddr, AttemptState>,
    max_failures: u32,
    window: Duration,
}

struct AttemptState {
    failures: u32,
    since: Instant,
    blocked_until: Option<Instant>,
}

impl LoginGuard {
    pub fn new(max_failures: u32, window: Duration) -> Self {
        Self {
            inner: DashMap::new(),
            max_failures,
            window,
        }
    }

    pub fn blocked_for(&self, ip: IpAddr) -> Option<Duration> {
        let now = Instant::now();
        let mut entry = self.inner.entry(ip).or_insert_with(|| AttemptState {
            failures: 0,
            since: now,
            blocked_until: None,
        });
        if let Some(until) = entry.blocked_until {
            if now < until {
                return Some(until.saturating_duration_since(now));
            }
            entry.failures = 0;
            entry.since = now;
            entry.blocked_until = None;
        }
        if now.duration_since(entry.since) >= self.window && entry.failures > 0 {
            entry.failures = 0;
            entry.since = now;
        }
        None
    }

    pub fn record_failure(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut entry = self.inner.entry(ip).or_insert_with(|| AttemptState {
            failures: 0,
            since: now,
            blocked_until: None,
        });
        if now.duration_since(entry.since) >= self.window {
            entry.failures = 0;
            entry.since = now;
        }
        entry.failures = entry.failures.saturating_add(1);
        if entry.failures >= self.max_failures {
            entry.blocked_until = Some(now + self.window);
        }
    }

    pub fn record_success(&self, ip: IpAddr) {
        self.inner.remove(&ip);
    }

    pub fn cleanup(&self) {
        let now = Instant::now();
        self.inner.retain(|_, entry| match entry.blocked_until {
            Some(until) => now < until,
            None => entry.failures > 0 && now.duration_since(entry.since) < self.window,
        });
    }
}

impl AppState {
    pub fn new(
        cfg: Arc<Config>,
        crypto: Crypto,
        credentials: Credentials,
        credentials_path: PathBuf,
    ) -> Self {
        let burst: f64 = 4.0 * 1024.0 * 1024.0;
        Self {
            upload_limiter: Arc::new(RateLimiter::new(
                cfg.upload_rate_bps,
                burst.max(cfg.upload_rate_bps),
            )),
            download_limiter: Arc::new(RateLimiter::new(
                cfg.download_rate_bps,
                burst.max(cfg.download_rate_bps),
            )),
            upload_sem: Arc::new(Semaphore::new(cfg.max_upload_concurrency)),
            download_sem: Arc::new(Semaphore::new(cfg.max_download_concurrency)),
            preview_sem: Arc::new(Semaphore::new(cfg.max_preview_concurrency)),
            login_sem: Arc::new(Semaphore::new(2)),
            credentials: RwLock::new(credentials),
            credentials_path,
            credentials_update: Mutex::new(()),
            sessions: DashMap::new(),
            preview_tokens: DashMap::new(),
            uploads: DashMap::new(),
            login_guard: LoginGuard::new(cfg.login_max_failures, cfg.login_window),
            cfg,
            crypto,
        }
    }
}
