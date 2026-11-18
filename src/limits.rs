//! Per-client rate limiting for the public API.
//!
//! Part of GR-2. A token bucket per client IP: [`BURST`] requests at once, refilled at [`PER_SECOND`].
//! The client IP is the first `X-Forwarded-For` entry, which Caddy sets and overwrites (clients
//! can't spoof it through the proxy). Without that header the peer address is used. Time comes from
//! the injected clock, so tests don't sleep.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;

use axum::extract::{ConnectInfo, Request};
use axum::http::HeaderMap;

/// Requests a client may make at once.
pub const BURST: u32 = 20;
/// Requests a client regains per second.
pub const PER_SECOND: u32 = 2;
/// Above this many tracked clients, full buckets are dropped.
const MAX_TRACKED: usize = 10_000;

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: u32,
    at: u64,
}

/// Token buckets keyed by client IP.
#[derive(Debug, Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    /// Takes one token for `ip` at `now` (Unix seconds); `false` when it has none left.
    pub fn allow(&self, ip: IpAddr, now: u64) -> bool {
        let Ok(mut buckets) = self.buckets.lock() else {
            // A poisoned lock means a panic elsewhere; fail open rather than take the API down.
            return true;
        };
        if buckets.len() >= MAX_TRACKED {
            buckets.retain(|_, b| refill(*b, now).tokens < BURST);
        }
        let bucket = buckets.entry(ip).or_insert(Bucket {
            tokens: BURST,
            at: now,
        });
        *bucket = refill(*bucket, now);
        if bucket.tokens == 0 {
            return false;
        }
        bucket.tokens -= 1;
        true
    }

    /// How many clients are tracked.
    pub fn tracked(&self) -> usize {
        self.buckets.lock().map_or(0, |b| b.len())
    }
}

fn refill(bucket: Bucket, now: u64) -> Bucket {
    let elapsed = now.saturating_sub(bucket.at);
    let gained = u32::try_from(elapsed.saturating_mul(u64::from(PER_SECOND))).unwrap_or(u32::MAX);
    Bucket {
        tokens: bucket.tokens.saturating_add(gained).min(BURST),
        at: now.max(bucket.at),
    }
}

/// The client's IP: the first `X-Forwarded-For` entry, else the peer address, else `0.0.0.0`.
pub fn client_ip(request: &Request) -> IpAddr {
    forwarded_for(request.headers())
        .or_else(|| {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip())
        })
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")?
        .to_str()
        .ok()?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    const A: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
    const B: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8));

    #[test]
    fn gr_2_allows_a_burst_then_refills_over_time() {
        let limiter = RateLimiter::default();
        for _ in 0..BURST {
            assert!(limiter.allow(A, 100));
        }
        assert!(!limiter.allow(A, 100), "the burst is spent");
        assert!(limiter.allow(B, 100), "other clients are unaffected");
        assert!(
            limiter.allow(A, 101) && limiter.allow(A, 101),
            "two tokens a second"
        );
        assert!(!limiter.allow(A, 101));
        for _ in 0..BURST {
            assert!(limiter.allow(A, 200));
        }
        assert!(!limiter.allow(A, 200), "refill stops at the burst size");
        assert!(
            limiter.allow(A, 50) || !limiter.allow(A, 50),
            "a clock going back never panics"
        );
    }

    #[test]
    fn gr_2_drops_idle_clients_when_many_are_tracked() {
        let limiter = RateLimiter::default();
        for n in 0..MAX_TRACKED {
            let ip = IpAddr::V4(Ipv4Addr::from(u32::try_from(n).unwrap()));
            assert!(limiter.allow(ip, 0));
        }
        assert_eq!(limiter.tracked(), MAX_TRACKED);
        // Twenty seconds later every bucket is full again, so all are dropped before A is added.
        assert!(limiter.allow(A, 20));
        assert_eq!(limiter.tracked(), 1);
    }

    #[test]
    fn gr_2_client_ip_prefers_the_proxy_header() {
        let mut request = Request::new(Body::empty());
        assert_eq!(client_ip(&request), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 2], 5000))));
        assert_eq!(client_ip(&request), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        request
            .headers_mut()
            .insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&request), A);
        request
            .headers_mut()
            .insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        assert_eq!(client_ip(&request), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    }
}
