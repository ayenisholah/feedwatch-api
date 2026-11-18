//! GR-2: the public API is read-only and bounded: GET only, CORS for the site origin only,
//! per-client rate limits, and a 30 s cache in front of InfluxDB.

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{FakeStore, NOW, ORIGIN, app, csv};
use feedwatch_api::limits::BURST;
use feedwatch_api::routes::{AppState, router};
use tower::ServiceExt;

async fn send(app: &Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.unwrap()
}

fn get_from(path: &str, ip: &str) -> Request<Body> {
    Request::get(path)
        .header("x-forwarded-for", ip)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn gr_2_only_get_is_allowed() {
    let app = app(FakeStore::new());
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        for path in ["/api/feeds", "/api/health", "/api/incidents"] {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                send(&app, request).await.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path}"
            );
        }
    }
}

#[tokio::test]
async fn gr_2_cors_allows_only_the_site_origin() {
    let app = app(FakeStore::new());
    let from = |origin: &str| {
        Request::get("/api/health")
            .header("origin", origin)
            .body(Body::empty())
            .unwrap()
    };
    let allowed = send(&app, from(ORIGIN)).await;
    assert_eq!(
        allowed.headers()["access-control-allow-origin"],
        ORIGIN,
        "the site gets CORS"
    );
    let foreign = send(&app, from("https://evil.example")).await;
    assert!(
        foreign
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );

    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("/api/feeds")
        .header("origin", ORIGIN)
        .header("access-control-request-method", "GET")
        .body(Body::empty())
        .unwrap();
    let response = send(&app, preflight).await;
    assert_eq!(response.status(), StatusCode::OK);
    let methods = response.headers()["access-control-allow-methods"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        methods.contains("GET") && !methods.contains("POST"),
        "{methods}"
    );
}

#[tokio::test]
async fn gr_2_rate_limits_each_client() {
    let app = app(FakeStore::new());
    let mut limited = 0;
    for _ in 0..BURST + 5 {
        let response = send(&app, get_from("/api/health", "203.0.113.7")).await;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(response.headers()["retry-after"], "1");
            limited += 1;
        }
    }
    assert_eq!(limited, 5, "the burst is {BURST}, then 429");
    let other = send(&app, get_from("/api/health", "203.0.113.8")).await;
    assert_eq!(
        other.status(),
        StatusCode::OK,
        "another client is unaffected"
    );
}

#[tokio::test]
async fn gr_2_caches_data_routes_for_30_seconds() {
    let store = FakeStore::new().on(
        "feed_round",
        &csv(&[(
            "feed_round",
            "base",
            "ETH",
            "",
            "2026-11-18T00:04:00Z",
            "value",
            "1",
        )]),
    );
    let now = Arc::new(AtomicU64::new(NOW));
    let clock = Arc::clone(&now);
    let app = router(AppState::with_clock(
        store.clone(),
        "feedwatch".into(),
        ORIGIN.into(),
        Arc::new(move || clock.load(Ordering::SeqCst)),
    ));

    let first = send(&app, get_from("/api/feeds", "198.51.100.1")).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["cache-control"], "public, max-age=30");
    assert_eq!(store.calls(), 1);

    now.store(NOW + 29, Ordering::SeqCst);
    let hit = send(&app, get_from("/api/feeds", "198.51.100.1")).await;
    assert_eq!(hit.status(), StatusCode::OK);
    assert_eq!(hit.headers()["content-type"], "application/json");
    assert_eq!(hit.headers()["cache-control"], "public, max-age=30");
    assert_eq!(store.calls(), 1, "served from the cache");

    send(&app, get_from("/api/feeds?chain=base", "198.51.100.1")).await;
    assert_eq!(store.calls(), 2, "the query string is part of the key");

    now.store(NOW + 31, Ordering::SeqCst);
    send(&app, get_from("/api/feeds", "198.51.100.1")).await;
    assert_eq!(store.calls(), 3, "expired after 30 s");

    // Errors and health aren't cached.
    send(&app, get_from("/api/feeds?chain=solana", "198.51.100.1")).await;
    send(&app, get_from("/api/feeds?chain=solana", "198.51.100.1")).await;
    send(&app, get_from("/api/health", "198.51.100.1")).await;
    send(&app, get_from("/api/health", "198.51.100.1")).await;
    assert_eq!(
        store.calls(),
        5,
        "two health queries, no data queries for 400s"
    );
    let health = send(&app, get_from("/api/health", "198.51.100.1")).await;
    assert!(health.headers().get("cache-control").is_none());
}
