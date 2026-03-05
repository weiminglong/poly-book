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
        let quota = Quota::per_second(NonZeroU32::new(150).unwrap())
            .allow_burst(NonZeroU32::new(150).unwrap());
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
