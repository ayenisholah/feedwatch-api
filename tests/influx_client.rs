//! FR-FW-6: the InfluxDB client sends Flux the way InfluxDB 2 expects and surfaces its failures.
//!
//! The wiremock tests run everywhere. The real InfluxDB 2.9.1 round trip needs Docker, so it's
//! `#[ignore]` locally; `CI=1 ./scripts/check.sh` runs it, as CI does.

// Test helpers outside #[test] functions may unwrap; a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

mod common;

use common::{TOKEN, influx, seed};
use feedwatch_api::influx::{HttpInflux, InfluxConfig, InfluxError, InfluxQuery};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(url: String) -> InfluxConfig {
    InfluxConfig {
        url,
        org: "feedwatch".to_owned(),
        bucket: "feedwatch".to_owned(),
        token: TOKEN.to_owned(),
    }
}

#[tokio::test]
async fn fr_fw_6_posts_flux_and_parses_the_csv() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/query"))
        .and(query_param("org", "feedwatch"))
        .and(header("authorization", "Token feedwatch-test-token"))
        .and(header("accept", "application/csv"))
        .and(body_partial_json(
            serde_json::json!({"query": "buckets()", "type": "flux"}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(",result,table,name\r\n,,0,feedwatch\r\n"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = HttpInflux::new(config(server.uri()), Duration::from_secs(2)).unwrap();
    let rows = client.query("buckets()").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "feedwatch");
}

#[tokio::test]
async fn fr_fw_6_returns_http_errors_as_typed_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized access"))
        .mount(&server)
        .await;

    let client = HttpInflux::new(config(server.uri()), Duration::from_secs(2)).unwrap();
    assert_eq!(
        client.query("buckets()").await,
        Err(InfluxError::Http {
            status: 401,
            body: "unauthorized access".to_owned()
        })
    );
}

#[tokio::test]
async fn fr_fw_6_times_out_and_reports_unreachable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;

    let client = HttpInflux::new(config(server.uri()), Duration::from_millis(100)).unwrap();
    assert!(matches!(
        client.query("buckets()").await,
        Err(InfluxError::Unreachable(_))
    ));
    assert_eq!(client.bucket(), "feedwatch");
}

#[tokio::test]
#[ignore = "needs Docker; run with CI=1 ./scripts/check.sh"]
async fn fr_fw_6_reads_contract_points_back_from_real_influxdb() {
    let (_container, url) = influx().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    seed(
        &url,
        &format!(
            "feed_round,chain=base,feed_id=ETH value=3120.5,answer_raw=\"312050000000\",round_id=\"7\",updated_at={now}i,age_s=12i {now}\n\
             incident,chain=ethereum,feed_id=USUAL,kind=error,event=open incident_id=\"ethereum-USUAL-1\",detail=\"reverted, InvalidLastUpdateDetails\" {now}"
        ),
    )
    .await;

    let client = HttpInflux::new(config(url), Duration::from_secs(5)).unwrap();
    let rows = client
        .query(&format!(
            r#"from(bucket: "{}") |> range(start: -1h) |> filter(fn: (r) => r._measurement == "feed_round" and r._field == "value")"#,
            client.bucket()
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["chain"], "base");
    assert_eq!(rows[0]["feed_id"], "ETH");
    assert_eq!(rows[0]["_value"], "3120.5");

    let rows = client
        .query(r#"from(bucket: "feedwatch") |> range(start: -1h) |> filter(fn: (r) => r._measurement == "incident" and r._field == "detail")"#)
        .await
        .unwrap();
    assert_eq!(rows[0]["_value"], "reverted, InvalidLastUpdateDetails");

    assert!(matches!(
        client.query("from(bucket: \"feedwatch\") |> nope()").await,
        Err(InfluxError::Http { status: 400, .. } | InfluxError::Query(_))
    ));
}
