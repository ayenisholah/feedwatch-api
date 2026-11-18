//! FR-FW-6: `/api/compare` puts the feeds present on all three chains side by side, with the spread.

#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::{FakeStore, app, csv, get};
use feedwatch_api::influx::InfluxError;
use serde_json::json;

const T: &str = "2026-11-18T00:04:00Z";

#[tokio::test]
async fn fr_fw_6_compare_lists_feeds_on_every_chain_with_spread() {
    let mut rows = Vec::new();
    for (chain, eth, btc) in [
        ("ethereum", "99", "60000"),
        ("arbitrum", "100", ""),
        ("base", "101", "60010"),
    ] {
        rows.push(("feed_round", chain, "ETH", "", T, "value", eth));
        rows.push(("feed_round", chain, "ETH", "", T, "age_s", "30"));
        rows.push(("feed_check", chain, "ETH", "ok", T, "heartbeat_s", "86400"));
        if !btc.is_empty() {
            rows.push(("feed_round", chain, "BTC", "", T, "value", btc));
        }
    }
    rows.push(("poll_cycle", "", "", "", T, "duration_ms", "280"));
    let store = FakeStore::new().on("feed_round", &csv(&rows));

    let (status, body) = get(app(store), "/api/compare").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["lastPollAt"], 1_794_960_240u64);
    let feeds = body["feeds"].as_array().unwrap();
    assert_eq!(feeds.len(), 1, "BTC isn't on Arbitrum: {body}");
    let eth = &feeds[0];
    assert_eq!(eth["feedId"], "ETH");
    assert!((eth["spreadPct"].as_f64().unwrap() - 2.0).abs() < 1e-9);
    assert_eq!(
        eth["chains"]["arbitrum"],
        json!({"value": 100.0, "ageS": 30, "status": "ok"})
    );
    let chains: Vec<&String> = eth["chains"].as_object().unwrap().keys().collect();
    assert_eq!(chains.len(), 3);
}

#[tokio::test]
async fn fr_fw_6_compare_is_503_when_influx_fails() {
    let store = FakeStore::new().fail("feed_round", InfluxError::Unreachable("down".into()));
    let (status, _) = get(app(store), "/api/compare").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
