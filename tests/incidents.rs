//! FR-FW-6: `/api/incidents` lists incidents from the poller's open and close events.
//!
//! The last test seeds a real InfluxDB 2.9.1 (Docker, `#[ignore]` locally; `CI=1 ./scripts/check.sh`
//! runs it).

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use common::{FakeStore, app, get, influx, seed, unix_now};
use feedwatch_api::influx::{HttpInflux, InfluxConfig, InfluxError};
use feedwatch_api::routes::{AppState, router};
use serde_json::json;

fn events_csv() -> String {
    let mut out = String::from(
        "#datatype,string,long,string,string,string,string,string,dateTime:RFC3339,string,string\r\n\
         ,result,table,_measurement,chain,feed_id,kind,event,_time,_field,_value\r\n",
    );
    for (i, (feed, kind, event, time, id, detail)) in [
        (
            "ETH",
            "stale",
            "open",
            "2026-11-18T00:00:00Z",
            "i-1",
            "age 90000s, heartbeat 86400s",
        ),
        (
            "ETH",
            "stale",
            "close",
            "2026-11-18T00:10:00Z",
            "i-1",
            "fresh",
        ),
        (
            "USUAL",
            "error",
            "open",
            "2026-11-18T01:00:00Z",
            "i-2",
            "reverted, \"InvalidLastUpdateDetails\"",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        for (field, value) in [("incident_id", id), ("detail", detail)] {
            out.push_str(&format!(
                ",_result,{i},incident,ethereum,{feed},{kind},{event},{time},{field},\"{}\"\r\n",
                value.replace('"', "\"\"")
            ));
        }
    }
    out
}

#[tokio::test]
async fn fr_fw_6_incidents_default_to_open_ones() {
    let store = FakeStore::new().on("incident", &events_csv());
    let (status, body) = get(app(Arc::clone(&store)), "/api/incidents").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"incidents": [{
            "id": "i-2", "chain": "ethereum", "feedId": "USUAL", "kind": "error",
            "openedAt": 1_794_963_600u64, "closedAt": null,
            "detail": "reverted, \"InvalidLastUpdateDetails\""
        }]})
    );
    let (_, open) = get(app(Arc::clone(&store)), "/api/incidents?status=open").await;
    assert_eq!(open, body);
}

#[tokio::test]
async fn fr_fw_6_incidents_all_includes_closed_newest_first() {
    let store = FakeStore::new().on("incident", &events_csv());
    let (status, body) = get(app(store), "/api/incidents?status=all").await;
    assert_eq!(status, StatusCode::OK);
    let list = body["incidents"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], "i-2");
    assert_eq!(list[1]["id"], "i-1");
    assert_eq!(list[1]["closedAt"], 1_794_960_600u64);
    assert_eq!(list[1]["detail"], "age 90000s, heartbeat 86400s");
}

#[tokio::test]
async fn fr_fw_6_incidents_reject_unknown_status_and_report_influx_failures() {
    let store = FakeStore::new();
    let (status, body) = get(app(Arc::clone(&store)), "/api/incidents?status=closed").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("open or all"));
    assert_eq!(store.calls(), 0);

    let store = FakeStore::new().fail("incident", InfluxError::Unreachable("down".into()));
    let (status, _) = get(app(store), "/api/incidents").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
#[ignore = "needs Docker; run with CI=1 ./scripts/check.sh"]
async fn fr_fw_6_incidents_from_real_influxdb() {
    let (_container, url) = influx().await;
    let now = unix_now();
    let (t1, t2, t3) = (now - 600, now - 300, now - 60);
    // Shaped like the poller's `incident` points (feedwatch-poller src/influx.rs).
    seed(
        &url,
        &format!(
            "incident,chain=base,feed_id=ETH,kind=stale,event=open incident_id=\"base-ETH-{t1}\",detail=\"age 90000s, heartbeat 86400s\" {t1}\n\
             incident,chain=base,feed_id=ETH,kind=stale,event=close incident_id=\"base-ETH-{t1}\",detail=\"\" {t2}\n\
             incident,chain=ethereum,feed_id=USUAL,kind=error,event=open incident_id=\"ethereum-USUAL-{t3}\",detail=\"reverted\" {t3}"
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

    let (status, body) = get(app.clone(), "/api/incidents").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let open = body["incidents"].as_array().unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["feedId"], "USUAL");
    assert_eq!(open[0]["openedAt"], t3);

    let (_, body) = get(app, "/api/incidents?status=all").await;
    let all = body["incidents"].as_array().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1]["openedAt"], t1);
    assert_eq!(all[1]["closedAt"], t2);
    assert_eq!(all[1]["detail"], "age 90000s, heartbeat 86400s");
}
