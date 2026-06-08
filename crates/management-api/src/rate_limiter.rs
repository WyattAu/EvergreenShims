use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use parking_lot::Mutex;
use tonic::body::BoxBody;
use tonic::Status;
use tower::{Layer, Service};

#[derive(Clone)]
struct RateLimiterInner {
    timestamps: VecDeque<std::time::Instant>,
    max_requests: usize,
    window: Duration,
}

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<RateLimiterInner>>,
}

impl RateLimiter {
    pub fn new(max_requests_per_minute: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RateLimiterInner {
                timestamps: VecDeque::new(),
                max_requests: max_requests_per_minute,
                window: Duration::from_secs(60),
            })),
        }
    }

    pub fn allow(&self) -> bool {
        let mut guard = self.inner.lock();
        let now = std::time::Instant::now();

        // Remove expired entries
        while let Some(&front) = guard.timestamps.front() {
            if now.duration_since(front) > guard.window {
                guard.timestamps.pop_front();
            } else {
                break;
            }
        }

        if guard.timestamps.len() >= guard.max_requests {
            return false;
        }

        guard.timestamps.push_back(now);
        true
    }

    #[cfg(test)]
    fn current_count(&self) -> usize {
        let mut guard = self.inner.lock();
        let now = std::time::Instant::now();
        while let Some(&front) = guard.timestamps.front() {
            if now.duration_since(front) > guard.window {
                guard.timestamps.pop_front();
            } else {
                break;
            }
        }
        guard.timestamps.len()
    }
}

#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: RateLimiter,
}

impl RateLimitLayer {
    pub fn new(max_requests_per_minute: usize) -> Self {
        Self {
            limiter: RateLimiter::new(max_requests_per_minute),
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: self.limiter.clone(),
        }
    }
}

#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: RateLimiter,
}

impl<S> Service<http::Request<BoxBody>> for RateLimitService<S>
where
    S: Service<http::Request<BoxBody>, Response = http::Response<BoxBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = http::Response<BoxBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<BoxBody>) -> Self::Future {
        if !self.limiter.allow() {
            let response = Status::resource_exhausted("rate limit exceeded").into_http();
            return Box::pin(async { Ok(response) });
        }

        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(req).await })
    }
}

pub fn rate_limit_from_env() -> usize {
    std::env::var("MGMT_RATE_LIMIT_RPM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new(5);
        for _ in 0..5 {
            assert!(limiter.allow());
        }
    }

    #[test]
    fn test_rate_limiter_rejects_over_limit() {
        let limiter = RateLimiter::new(3);
        assert!(limiter.allow());
        assert!(limiter.allow());
        assert!(limiter.allow());
        assert!(!limiter.allow());
    }

    #[test]
    fn test_rate_limiter_reset_after_window() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.allow());
        assert!(limiter.allow());
        assert!(!limiter.allow());
        // Manually clear timestamps to simulate window expiry
        limiter.inner.lock().timestamps.clear();
        assert!(limiter.allow());
    }

    #[test]
    fn test_rate_limiter_thread_safety() {
        let limiter = RateLimiter::new(100);
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let limiter = limiter.clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        limiter.allow();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(limiter.current_count(), 100);
    }

    #[test]
    fn test_rate_limit_from_env_default() {
        std::env::remove_var("MGMT_RATE_LIMIT_RPM");
        assert_eq!(rate_limit_from_env(), 60);
    }

    #[test]
    fn test_rate_limit_from_env_custom() {
        std::env::set_var("MGMT_RATE_LIMIT_RPM", "120");
        assert_eq!(rate_limit_from_env(), 120);
        std::env::remove_var("MGMT_RATE_LIMIT_RPM");
    }

    #[test]
    fn test_rate_limit_layer_clones() {
        let layer = RateLimitLayer::new(60);
        let _cloned = layer.clone();
    }
}
