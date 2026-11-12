use std::{
    collections::{HashSet, VecDeque},
    net::IpAddr,
    path::PathBuf,
    sync::{atomic::AtomicU64, Arc, Mutex as StdMutex, RwLock as StdRwLock},
    time::{Duration, Instant},
};

use dashmap::DashMap;
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::{
    config::Config, credentials::Credentials, crypto::Crypto, files::CachedListing,
    rate::RateLimiter,
};

pub struct AppState {
    pub cfg: Arc<Config>,
    pub crypto: Crypto,
    pub credentials: RwLock<Credentials>,
    pub credentials_path: PathBuf,
    pub credentials_update: Mutex<()>,
    pub sessions: DashMap<String, Session>,
    pub listings: DashMap<PathBuf, CachedListing>,
    /// Mutable in desktop local mode (switch between home, filesystem root,
    /// drives); fixed to `cfg.data_root` for the standalone server.
    current_root: StdRwLock<PathBuf>,
    pub login_guard: LoginGuard,
    pub login_challenges: LoginChallenges,
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
    /// Nonces of already accepted API envelopes. Shared with `AuthSession`
    /// clones so every handler sees the same replay window.
    pub replay: Arc<StdMutex<ReplayWindow>>,
}

/// Bounded set of AES-GCM nonces already accepted for one session.
///
/// AAD binds a ciphertext to its endpoint, but not to a point in time: without
/// this window an intercepted envelope could be POSTed again while the session
/// is still alive. Every envelope already carries a fresh random 12-byte
/// nonce, so rejecting a repeat needs no protocol change on the client side.
pub struct ReplayWindow {
    seen: HashSet<[u8; 12]>,
    order: VecDeque<([u8; 12], Instant)>,
    ttl: Duration,
    capacity: usize,
}

impl ReplayWindow {
    const CAPACITY: usize = 8_192;

    pub fn new(ttl: Duration) -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            ttl,
            capacity: Self::CAPACITY,
        }
    }

    /// Records `nonce` and returns false when it was already used.
    pub fn accept(&mut self, nonce: [u8; 12]) -> bool {
        self.prune_expired();
        // Check before evicting: otherwise a repeat of the oldest nonce could
        // be evicted first and then look fresh again.
        if self.seen.contains(&nonce) {
            return false;
        }
        while self.order.len() >= self.capacity {
            let Some((old, _)) = self.order.pop_front() else {
                break;
            };
            self.seen.remove(&old);
        }
        self.seen.insert(nonce);
        self.order.push_back((nonce, Instant::now()));
        true
    }

    pub fn prune(&mut self) {
        self.prune_expired();
    }

    fn prune_expired(&mut self) {
        let now = Instant::now();
        while let Some((_, inserted)) = self.order.front() {
            if now.duration_since(*inserted) <= self.ttl {
                break;
            }
            let Some((old, _)) = self.order.pop_front() else {
                break;
            };
            self.seen.remove(&old);
        }
    }
}

/// One-shot login challenges. A captured login envelope can only be replayed
/// if its challenge is still unused, which makes the request single-use.
pub struct LoginChallenges {
    inner: DashMap<String, Instant>,
    ttl: Duration,
    capacity: usize,
}

impl LoginChallenges {
    const CAPACITY: usize = 4_096;

    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: DashMap::new(),
            ttl,
            capacity: Self::CAPACITY,
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Returns None when too many challenges are outstanding (flood guard).
    pub fn issue(&self) -> Option<String> {
        self.prune();
        if self.inner.len() >= self.capacity {
            return None;
        }
        let value = crate::crypto::random_b64(32);
        self.inner.insert(value.clone(), Instant::now());
        Some(value)
    }

    pub fn consume(&self, value: &str) -> bool {
        let Some((_, issued)) = self.inner.remove(value) else {
            return false;
        };
        Instant::now().duration_since(issued) <= self.ttl
    }

    pub fn cleanup(&self) {
        self.prune();
    }

    fn prune(&self) {
        let now = Instant::now();
        self.inner
            .retain(|_, issued| now.duration_since(*issued) <= self.ttl);
    }
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
    pub fn data_root(&self) -> PathBuf {
        self.current_root
            .read()
            .expect("data root lock poisoned")
            .clone()
    }

    pub fn set_data_root(&self, root: PathBuf) {
        *self.current_root.write().expect("data root lock poisoned") = root;
        self.listings.clear();
    }

    pub fn new(
        cfg: Arc<Config>,
        crypto: Crypto,
        credentials: Credentials,
        credentials_path: PathBuf,
    ) -> Self {
        let burst: f64 = 4.0 * 1024.0 * 1024.0;
        let state = Self {
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
            listings: DashMap::new(),
            current_root: StdRwLock::new(cfg.data_root.clone()),
            uploads: DashMap::new(),
            login_guard: LoginGuard::new(cfg.login_max_failures, cfg.login_window),
            login_challenges: LoginChallenges::new(Duration::from_secs(120)),
            cfg,
            crypto,
        };
        if state.cfg.local_mode {
            let now = Instant::now();
            state.sessions.insert(
                crate::auth::LOCAL_SESSION_TOKEN.to_string(),
                Session {
                    key: crate::crypto::random_key(),
                    user: "local".to_string(),
                    expires_at: now + Duration::from_secs(365 * 24 * 3600),
                    hard_expires_at: now + Duration::from_secs(3650 * 24 * 3600),
                    replay: Arc::new(StdMutex::new(ReplayWindow::new(state.cfg.session_max_ttl))),
                },
            );
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_window_rejects_a_reused_nonce() {
        let mut window = ReplayWindow::new(Duration::from_secs(60));
        assert!(window.accept([1u8; 12]));
        assert!(!window.accept([1u8; 12]));
        assert!(window.accept([2u8; 12]));
        window.prune();
        assert!(!window.accept([1u8; 12]));
    }

    #[test]
    fn replay_window_evicts_the_oldest_nonce_when_full() {
        let mut window = ReplayWindow::new(Duration::from_secs(60));
        window.capacity = 2;
        assert!(window.accept([1u8; 12]));
        assert!(window.accept([2u8; 12]));
        assert!(!window.accept([1u8; 12]));
        assert!(!window.accept([2u8; 12]));
        assert!(window.accept([3u8; 12]));
        assert!(!window.accept([2u8; 12]));
        // A bounded window can only remember the most recent nonces.
        assert!(window.accept([1u8; 12]));
    }

    #[test]
    fn login_challenges_are_single_use() {
        let challenges = LoginChallenges::new(Duration::from_secs(120));
        let value = challenges.issue().expect("challenge issued");
        assert!(!challenges.consume("unknown"));
        assert!(challenges.consume(&value));
        assert!(!challenges.consume(&value));
        challenges.cleanup();
    }
}
