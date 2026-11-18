//! FR-FW-6 contract: `openapi.json` at the repo root is the API's published contract
//! (feedwatch-platform `docs/DATA_CONTRACT.md`). It must match what the code generates, and the
//! running API serves the same document.
//!
//! After changing a route or response type, regenerate it with
//! `UPDATE_OPENAPI=1 cargo test --test openapi` and commit the result.

#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::{FakeStore, app, get};
use feedwatch_api::routes::ApiDoc;
use utoipa::OpenApi;

fn committed_path() -> String {
    format!("{}/openapi.json", env!("CARGO_MANIFEST_DIR"))
}

fn generated() -> String {
    ApiDoc::openapi().to_pretty_json().unwrap() + "\n"
}

#[test]
fn fr_fw_6_openapi_matches_committed() {
    let generated = generated();
    if std::env::var("UPDATE_OPENAPI").is_ok_and(|v| v == "1") {
        std::fs::write(committed_path(), &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(committed_path()).unwrap_or_default();
    assert!(
        committed == generated,
        "openapi.json is out of date: run `UPDATE_OPENAPI=1 cargo test --test openapi` and commit it"
    );
}

#[test]
fn fr_fw_6_openapi_documents_every_route() {
    let doc: serde_json::Value = serde_json::from_str(&generated()).unwrap();
    let mut paths: Vec<&str> = doc["paths"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "/api/compare",
            "/api/feeds",
            "/api/feeds/{chain}/{feedId}",
            "/api/health",
            "/api/incidents"
        ]
    );
    for schema in [
        "FeedSummary",
        "FeedDetail",
        "CompareRow",
        "Incident",
        "Health",
        "ErrorBody",
    ] {
        assert!(
            doc["components"]["schemas"][schema].is_object(),
            "{schema} missing"
        );
    }
}

#[tokio::test]
async fn fr_fw_6_serves_the_openapi_document() {
    let (status, body) = get(app(FakeStore::new()), "/api/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["info"]["title"], "Feedwatch API");
    assert_eq!(
        body,
        serde_json::from_str::<serde_json::Value>(&generated()).unwrap()
    );
}
