//! FR-FW-6: the InfluxDB client sends Flux the way InfluxDB 2 expects and surfaces its failures.
//!
//! The wiremock tests run everywhere. The real InfluxDB 2.9.1 round trip needs Docker, so it's
//! `#[ignore]` locally; `CI=1 ./scripts/check.sh` runs it, as CI does.

// Test helpers outside #[test] functions may unwrap; a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use feedwatch_api::influx::{HttpInflux, InfluxConfig, InfluxError, InfluxQuery};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "feedwatch-test-token";

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

async fn influx() -> (ContainerAsync<GenericImage>, String) {
    let container = GenericImage::new("influxdb", "2.9.1")
        .with_exposed_port(8086.tcp())
        .with_wait_for(WaitFor::Nothing)
        .with_env_var("DOCKER_INFLUXDB_INIT_MODE", "setup")
        .with_env_var("DOCKER_INFLUXDB_INIT_USERNAME", "feedwatch")
        .with_env_var("DOCKER_INFLUXDB_INIT_PASSWORD", "feedwatch-test")
        .with_env_var("DOCKER_INFLUXDB_INIT_ORG", "feedwatch")
        .with_env_var("DOCKER_INFLUXDB_INIT_BUCKET", "feedwatch")
        .with_env_var("DOCKER_INFLUXDB_INIT_ADMIN_TOKEN", TOKEN)
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(8086).await.unwrap();
    (container, format!("http://127.0.0.1:{port}"))
}

/// Writes line protocol, retrying while setup mode restarts influxd.
async fn seed(url: &str, lines: &str) {
    let client = reqwest::Client::new();
    for _ in 0..60 {
        let sent = client
            .post(format!("{url}/api/v2/write"))
            .query(&[
                ("org", "feedwatch"),
                ("bucket", "feedwatch"),
                ("precision", "s"),
            ])
            .header("authorization", format!("Token {TOKEN}"))
            .body(lines.to_owned())
            .send()
            .await;
        if sent.is_ok_and(|r| r.status().as_u16() == 204) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("InfluxDB never accepted a write");
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
