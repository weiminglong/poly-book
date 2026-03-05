use governor::{
    clock::DefaultClock,
    middleware::NoOpMiddleware,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovRateLimiter,
};
use std::num::NonZeroU32;
use std::sync::Arc;

type Inner = GovRateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Inner>,
}

impl RateLimiter {
    pub fn new() -> Self {
        // Default: 1500 requests per 10 seconds = 150 req/s
        Self::with_window(1500, 10)
    }

    /// Create a rate limiter from total requests allowed over a window in seconds.
    /// E.g., `with_window(1500, 10)` = 150 req/s with burst up to 150.
    pub fn with_window(requests: u32, window_secs: u32) -> Self {
        let per_second = (requests / window_secs.max(1)).max(1);
        let quota = Quota::per_second(NonZeroU32::new(per_second).unwrap())
            .allow_burst(NonZeroU32::new(per_second).unwrap());
        Self {
            inner: Arc::new(GovRateLimiter::direct(quota)),
        }
    }

    pub async fn acquire(&self) {
        self.inner.until_ready().await;
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_with_window_allows_burst() {
        // 100 requests per 10 seconds = 10 req/s, burst of 10
        let limiter = RateLimiter::with_window(100, 10);
        // Should be able to acquire burst immediately
        for _ in 0..10 {
            limiter.acquire().await;
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_default_is_150_per_sec() {
        // Default: 1500/10 = 150 req/s
        let limiter = RateLimiter::new();
        // Should allow burst of 150 immediately
        for _ in 0..150 {
            limiter.acquire().await;
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_with_window_min_one() {
        // Edge case: 0 requests / 0 window should clamp to 1 req/s
        let limiter = RateLimiter::with_window(0, 0);
        limiter.acquire().await;
    }
}
