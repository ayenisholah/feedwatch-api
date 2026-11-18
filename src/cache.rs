//! 30-second response cache for the read routes, so a busy page can't hammer InfluxDB.
//!
//! Part of GR-2. Keyed by path and query; only 200 responses are kept. It holds at most
//! [`MAX_ENTRIES`] responses, dropping expired ones first and skipping new ones when still full.
//! Time comes from the injected clock.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::body::Bytes;
use axum::http::HeaderValue;

/// How long a response is reused, in seconds. Also sent as `Cache-Control: max-age`.
pub const TTL_S: u64 = 30;
/// The most responses kept.
pub const MAX_ENTRIES: usize = 1_000;

/// A stored response body and its content type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    /// When it was stored, Unix seconds.
    pub stored_at: u64,
    /// The `Content-Type` to send with it.
    pub content_type: Option<HeaderValue>,
    /// The response body.
    pub body: Bytes,
}

/// Responses by path and query.
#[derive(Debug, Default)]
pub struct ResponseCache {
    entries: Mutex<HashMap<String, Cached>>,
}

impl ResponseCache {
    /// The response stored under `key`, if it's younger than [`TTL_S`] at `now`.
    pub fn get(&self, key: &str, now: u64) -> Option<Cached> {
        let entries = self.entries.lock().ok()?;
        entries.get(key).filter(|c| fresh(c, now)).cloned()
    }

    /// Stores a response under `key`.
    pub fn put(&self, key: String, cached: Cached) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if entries.len() >= MAX_ENTRIES && !entries.contains_key(&key) {
            let now = cached.stored_at;
            entries.retain(|_, c| fresh(c, now));
            if entries.len() >= MAX_ENTRIES {
                return;
            }
        }
        entries.insert(key, cached);
    }

    /// How many responses are stored, fresh or not.
    pub fn len(&self) -> usize {
        self.entries.lock().map_or(0, |e| e.len())
    }

    /// Whether nothing is stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn fresh(cached: &Cached, now: u64) -> bool {
    now.saturating_sub(cached.stored_at) < TTL_S
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(at: u64, body: &'static str) -> Cached {
        Cached {
            stored_at: at,
            content_type: Some(HeaderValue::from_static("application/json")),
            body: Bytes::from_static(body.as_bytes()),
        }
    }

    #[test]
    fn gr_2_serves_a_response_for_30_seconds() {
        let cache = ResponseCache::default();
        assert!(cache.is_empty());
        cache.put("/api/feeds".into(), cached(100, "a"));
        assert_eq!(cache.get("/api/feeds", 129).unwrap().body, "a");
        assert_eq!(cache.get("/api/feeds", 130), None);
        assert_eq!(cache.get("/api/feeds?chain=base", 100), None);
        cache.put("/api/feeds".into(), cached(130, "b"));
        assert_eq!(cache.get("/api/feeds", 130).unwrap().body, "b");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn gr_2_cache_size_is_bounded() {
        let cache = ResponseCache::default();
        for n in 0..MAX_ENTRIES {
            cache.put(format!("/k{n}"), cached(0, "x"));
        }
        cache.put("/full".into(), cached(10, "x"));
        assert_eq!(
            cache.get("/full", 10),
            None,
            "all fresh: the new one is skipped"
        );
        cache.put("/k0".into(), cached(10, "y"));
        assert_eq!(
            cache.get("/k0", 10).unwrap().body,
            "y",
            "existing keys still update"
        );
        cache.put("/later".into(), cached(40, "z"));
        assert_eq!(
            cache.get("/later", 40).unwrap().body,
            "z",
            "expired entries make room"
        );
        assert_eq!(cache.len(), 1);
    }
}
