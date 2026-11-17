//! FR-FW-6: the API reads the InfluxDB data contract it was built against.
//!
//! The contract lives in feedwatch-platform `contracts/influx-schema.json`. A copy is vendored at
//! `tests/fixtures/influx-schema.json` so CI, which checks out only this repo, can test against it.
//! When the platform repo sits next to this one, the copy must match it.

// Test helpers outside #[test] functions may unwrap; a failure is a test failure.
#![allow(clippy::unwrap_used)]

use serde_json::Value;

fn contract() -> Value {
    let path = format!(
        "{}/tests/fixtures/influx-schema.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn fr_fw_6_built_against_contract_version_2() {
    let contract = contract();
    assert_eq!(contract["version"], 2);
    for measurement in ["feed_round", "feed_check", "incident", "poll_cycle"] {
        assert!(
            contract["measurements"][measurement].is_object(),
            "{measurement} missing from the contract"
        );
    }
}

#[test]
fn vendored_contract_matches_the_platform_repo() {
    let platform = format!(
        "{}/../feedwatch-platform/contracts/influx-schema.json",
        env!("CARGO_MANIFEST_DIR")
    );
    match std::fs::read_to_string(platform) {
        Ok(text) => assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            contract(),
            "refresh tests/fixtures"
        ),
        Err(_) => eprintln!(
            "feedwatch-platform not checked out next to this repo; using the vendored contract"
        ),
    }
}
