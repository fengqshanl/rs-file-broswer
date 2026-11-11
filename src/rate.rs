use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Global token-bucket limiter shared by all transfers in one direction.
/// The bucket is allowed to go negative so callers queue fairly without
/// deadlocking on chunks larger than the burst capacity.
pub struct RateLimiter {
    rate: f64,
    burst: f64,
    inner: Mutex<Bucket>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(bytes_per_second: f64, burst: f64) -> Self {
        Self {
            rate: bytes_per_second,
            burst: burst.max(1.0),
            inner: Mutex::new(Bucket {
                tokens: burst.max(1.0),
                last: Instant::now(),
            }),
        }
    }

    pub async fn wait(&self, bytes: usize) {
        if self.rate <= 0.0 || bytes == 0 {
            return;
        }
        let need = bytes as f64;
        let sleep_for = {
            let mut bucket = self.inner.lock().await;
            let now = Instant::now();
            let elapsed = now.duration_since(bucket.last).as_secs_f64();
            bucket.last = now;
            bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.burst);
            bucket.tokens -= need;
            if bucket.tokens >= 0.0 {
                0.0
            } else {
                -bucket.tokens / self.rate
            }
        };
        if sleep_for > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(sleep_for)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limiter_delays_after_burst() {
        let limiter = RateLimiter::new(100_000.0, 100_000.0);
        let started = Instant::now();
        limiter.wait(100_000).await;
        assert!(started.elapsed() < Duration::from_millis(200));
        limiter.wait(100_000).await;
        assert!(started.elapsed() >= Duration::from_millis(850));
    }
}
