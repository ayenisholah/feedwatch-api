//! NFR-2: the API reports its own health, InfluxDB's, and whether the poller is still writing,
//! within 1 s.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use common::{FakeStore, NOW, app, csv, get};
use feedwatch_api::influx::InfluxError;
use serde_json::json;

fn poll_at(time: &str) -> String {
    csv(&[("poll_cycle", "", "", "", time, "duration_ms", "280")])
}

#[tokio::test]
async fn nfr_2_health_is_ok_with_a_recent_poll() {
    // NOW is 00:05:00; the last poll was 60 s earlier.
    let store = FakeStore::new().on("poll_cycle", &poll_at("2026-11-18T00:04:00Z"));
    let (status, body) = get(app(store), "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"status": "ok", "influx": "ok", "lastPollAt": NOW - 60})
    );
}

#[tokio::test]
async fn nfr_2_health_is_degraded_when_the_poller_stops() {
    let store = FakeStore::new().on("poll_cycle", &poll_at("2026-11-18T00:01:59Z"));
    let (status, body) = get(app(store), "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["influx"], "ok");

    let (_, body) = get(app(FakeStore::new()), "/api/health").await;
    assert_eq!(
        body,
        json!({"status": "degraded", "influx": "ok", "lastPollAt": null})
    );
}

#[tokio::test]
async fn nfr_2_health_reports_influx_down_on_error() {
    let store = FakeStore::new().fail("poll_cycle", InfluxError::Unreachable("refused".into()));
    let (status, body) = get(app(store), "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"status": "degraded", "influx": "down", "lastPollAt": null})
    );
}

#[tokio::test]
async fn nfr_2_health_answers_within_1_s_when_influx_hangs() {
    let store = FakeStore::new().slow(Duration::from_secs(5));
    let started = Instant::now();
    let (status, body) = get(app(store), "/api/health").await;
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["influx"], "down");
}

#[tokio::test]
async fn gr_2_unknown_routes_are_not_found() {
    let (status, _) = get(app(FakeStore::new()), "/api/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
