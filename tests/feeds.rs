//! FR-FW-6: `/api/feeds` and `/api/feeds/{chain}/{feedId}` serve the poller's latest data.
//!
//! Most tests use a fake store answering with annotated CSV. The last one seeds a real InfluxDB 2.9.1
//! (Docker, `#[ignore]` locally; `CI=1 ./scripts/check.sh` runs it).

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use common::{FakeStore, app, csv, get, influx, seed, unix_now};
use feedwatch_api::influx::{HttpInflux, InfluxConfig, InfluxError};
use feedwatch_api::routes::{AppState, router};
use serde_json::json;

const T: &str = "2026-11-18T00:04:00Z";
const T_UNIX: u64 = 1_794_960_240;

#[allow(clippy::too_many_lines)] // one fixture row per line
fn latest_csv() -> String {
    csv(&[
        ("feed_round", "base", "ETH", "", T, "value", "3120.5"),
        (
            "feed_round",
            "base",
            "ETH",
            "",
            T,
            "answer_raw",
            "312050000000",
        ),
        ("feed_round", "base", "ETH", "", T, "round_id", "7"),
        (
            "feed_round",
            "base",
            "ETH",
            "",
            T,
            "updated_at",
            "1794960000",
        ),
        ("feed_round", "base", "ETH", "", T, "age_s", "240"),
        ("feed_check", "base", "ETH", "ok", T, "heartbeat_s", "86400"),
        (
            "feed_check",
            "base",
            "ETH",
            "ok",
            T,
            "deviation_threshold_pct",
            "0.5",
        ),
        (
            "feed_check",
            "base",
            "ETH",
            "ok",
            T,
            "reference_deviation_pct",
            "0.02",
        ),
        (
            "feed_check",
            "base",
            "ETH",
            "ok",
            T,
            "reference_source",
            "chainlink",
        ),
        (
            "feed_check",
            "ethereum",
            "USUAL",
            "error",
            T,
            "heartbeat_s",
            "86400",
        ),
        ("poll_cycle", "", "", "", T, "duration_ms", "280"),
    ])
}

#[tokio::test]
async fn fr_fw_6_feeds_lists_the_latest_round_and_check_per_feed() {
    let store = FakeStore::new().on("feed_round", &latest_csv());
    let (status, body) = get(app(store), "/api/feeds").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["lastPollAt"], T_UNIX);
    let feeds = body["feeds"].as_array().unwrap();
    assert_eq!(feeds.len(), 2);
    // Ethereum sorts first.
    assert_eq!(
        feeds[0],
        json!({
            "chain": "ethereum", "feedId": "USUAL", "status": "error", "checkedAt": T_UNIX,
            "value": null, "answerRaw": null, "roundId": null, "updatedAt": null, "ageS": null,
            "heartbeatS": 86400, "deviationThresholdPct": null, "referenceDeviationPct": null,
            "referenceSource": null
        })
    );
    assert_eq!(
        feeds[1],
        json!({
            "chain": "base", "feedId": "ETH", "status": "ok", "checkedAt": T_UNIX,
            "value": 3120.5, "answerRaw": "312050000000", "roundId": "7",
            "updatedAt": 1_794_960_000u64, "ageS": 240, "heartbeatS": 86400,
            "deviationThresholdPct": 0.5, "referenceDeviationPct": 0.02,
            "referenceSource": "chainlink"
        })
    );
}

#[tokio::test]
async fn fr_fw_6_feeds_filters_by_chain_and_rejects_unknown_chains() {
    let store = FakeStore::new().on(r#"r.chain == "base""#, &latest_csv());
    let (status, _) = get(app(Arc::clone(&store)), "/api/feeds?chain=base").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(store.calls(), 1);

    let (status, body) = get(app(Arc::clone(&store)), "/api/feeds?chain=solana").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("unknown chain"));
    assert_eq!(store.calls(), 1, "no query for a bad chain");

    let (status, body) = get(app(FakeStore::new()), "/api/feeds?chain=").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"lastPollAt": null, "feeds": []}));
}

#[tokio::test]
async fn fr_fw_6_influx_failures_are_503_without_detail() {
    let store = FakeStore::new().fail(
        "feed_round",
        InfluxError::Unreachable("secret-host:8086".into()),
    );
    let (status, body) = get(app(store), "/api/feeds").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        json!({"error": "feed data is temporarily unavailable"})
    );
}

#[tokio::test]
async fn fr_fw_6_feed_detail_has_latest_and_a_24_h_series() {
    let series = csv(&[
        (
            "feed_round",
            "base",
            "ETH",
            "",
            "2026-11-18T00:00:00Z",
            "value",
            "3100",
        ),
        (
            "feed_round",
            "base",
            "ETH",
            "",
            "2026-11-18T00:00:00Z",
            "age_s",
            "0",
        ),
        (
            "feed_round",
            "base",
            "ETH",
            "",
            "2026-11-18T00:05:00Z",
            "value",
            "3120.5",
        ),
        (
            "feed_round",
            "base",
            "ETH",
            "",
            "2026-11-18T00:05:00Z",
            "age_s",
            "240",
        ),
    ]);
    let store = FakeStore::new()
        .on("aggregateWindow", &series)
        .on(r#"r.feed_id == "ETH""#, &latest_csv());
    let (status, body) = get(app(store), "/api/feeds/base/ETH").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["latest"]["feedId"], "ETH");
    assert_eq!(body["latest"]["value"], 3120.5);
    assert_eq!(
        body["series"],
        json!([
            {"t": 1_794_960_000u64, "value": 3100.0, "ageS": 0},
            {"t": 1_794_960_300u64, "value": 3120.5, "ageS": 240}
        ])
    );
}

#[tokio::test]
async fn fr_fw_6_feed_detail_rejects_bad_input_before_querying() {
    let store = FakeStore::new();
    let (status, _) = get(app(Arc::clone(&store)), "/api/feeds/solana/ETH").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = get(
        app(Arc::clone(&store)),
        "/api/feeds/base/ETH%22)%20or%20true",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("no data"));
    assert_eq!(store.calls(), 0, "invalid input never reaches Flux");

    let (status, _) = get(app(Arc::clone(&store)), "/api/feeds/base/NOPE").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(store.calls(), 2);
}

#[tokio::test]
async fn fr_fw_6_feed_detail_accepts_manifest_ids_with_slash_and_plus() {
    // 19 of the 146 manifest keys use `/` or `+`, such as WBTC/BTC and CASH+_FUNDAMENTAL. The web
    // client sends them percent-encoded in one path segment.
    let store = FakeStore::new().on(
        r#"r.feed_id == "WBTC/BTC""#,
        &csv(&[(
            "feed_round",
            "ethereum",
            "WBTC/BTC",
            "",
            T,
            "value",
            "1.0001",
        )]),
    );
    let (status, body) = get(app(Arc::clone(&store)), "/api/feeds/ethereum/WBTC%2FBTC").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["latest"]["feedId"], "WBTC/BTC");

    let (status, _) = get(
        app(Arc::clone(&store)),
        "/api/feeds/ethereum/CASH%2B_FUNDAMENTAL",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "valid ID, no data");
    assert_eq!(store.calls(), 4, "both IDs reached the store");
}

#[tokio::test]
async fn fr_fw_6_feed_detail_is_503_when_either_query_fails() {
    // Both queries fail.
    let store = FakeStore::new().fail("feed_round", InfluxError::Query("boom".into()));
    let (status, _) = get(app(store), "/api/feeds/base/ETH").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    // Only the series fails. Rules match in order, and the latest query also mentions
    // "feed_round", so the series rule goes first.
    let store = FakeStore::new()
        .fail("aggregateWindow", InfluxError::Query("boom".into()))
        .on("feed_round", &latest_csv());
    let (status, _) = get(app(store), "/api/feeds/base/ETH").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
#[ignore = "needs Docker; run with CI=1 ./scripts/check.sh"]
async fn fr_fw_6_serves_seeded_points_from_real_influxdb() {
    let (_container, url) = influx().await;
    let now = unix_now();
    let earlier = now - 60;
    // Two cycles shaped like the poller's line protocol. In the newer one ETH lost its reference
    // fields, and USUAL only ever has a check (its read reverts).
    seed(
        &url,
        &format!(
            "feed_round,chain=base,feed_id=ETH value=3100,answer_raw=\"310000000000\",round_id=\"6\",updated_at={earlier}i,age_s=0i {earlier}\n\
             feed_check,chain=base,feed_id=ETH,status=drift heartbeat_s=86400i,deviation_threshold_pct=0.5,reference_deviation_pct=0.9,reference_source=\"coinbase\" {earlier}\n\
             feed_round,chain=base,feed_id=ETH value=3120.5,answer_raw=\"312050000000\",round_id=\"7\",updated_at={now}i,age_s=0i {now}\n\
             feed_check,chain=base,feed_id=ETH,status=ok heartbeat_s=86400i,deviation_threshold_pct=0.5 {now}\n\
             feed_check,chain=ethereum,feed_id=USUAL,status=error heartbeat_s=86400i {now}\n\
             poll_cycle duration_ms=280i,feeds=2i,failed_feeds=1i {now}"
        ),
    )
    .await;
    let store = HttpInflux::new(
        InfluxConfig {
            url,
            org: "feedwatch".into(),
            bucket: "feedwatch".into(),
            token: common::TOKEN.into(),
        },
        Duration::from_secs(5),
    )
    .unwrap();
    let app = router(AppState::new(
        Arc::new(store),
        "feedwatch".into(),
        common::ORIGIN.into(),
    ));

    let (status, body) = get(app.clone(), "/api/feeds").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["lastPollAt"], now);
    let feeds = body["feeds"].as_array().unwrap();
    assert_eq!(feeds.len(), 2);
    assert_eq!(feeds[0]["feedId"], "USUAL");
    assert_eq!(feeds[0]["status"], "error");
    assert_eq!(feeds[1]["status"], "ok");
    assert_eq!(feeds[1]["value"], 3120.5);
    assert_eq!(feeds[1]["referenceDeviationPct"], json!(null));

    let (status, body) = get(app.clone(), "/api/feeds?chain=base").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["feeds"].as_array().unwrap().len(), 1);

    let (status, body) = get(app.clone(), "/api/feeds/base/ETH").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["latest"]["roundId"], "7");
    let series = body["series"].as_array().unwrap();
    assert!(!series.is_empty() && series.len() <= 2, "{series:?}");
    assert_eq!(series.last().unwrap()["value"], 3120.5);

    let (status, body) = get(app.clone(), "/api/health").await;
    assert_eq!(
        body,
        json!({"status": "ok", "influx": "ok", "lastPollAt": now}),
        "{status}"
    );

    let (status, _) = get(app, "/api/feeds/base/NOPE").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
