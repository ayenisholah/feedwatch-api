//! Shared test helpers: a fake [`InfluxQuery`], request helpers, and a real InfluxDB in Docker.

// Not every test file uses every helper.
#![allow(dead_code, clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use feedwatch_api::influx::{BoxFuture, InfluxError, InfluxQuery, Row, parse_annotated_csv};
use feedwatch_api::routes::{AppState, router};
use http_body_util::BodyExt;
use serde_json::Value;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tower::ServiceExt;

/// Now, for every fake-store test: 2026-11-18T00:05:00Z.
pub const NOW: u64 = 1_794_960_300;

type Answer = Result<String, InfluxError>;

/// Answers each query with the first rule whose needle the Flux contains. CSV answers are parsed
/// with the real parser, so fixtures look like what InfluxDB sends.
#[derive(Default)]
pub struct FakeStore {
    rules: Mutex<Vec<(String, Answer)>>,
    delay: Mutex<Option<Duration>>,
    pub calls: AtomicUsize,
}

impl FakeStore {
    pub fn new() -> Arc<FakeStore> {
        Arc::new(FakeStore::default())
    }

    /// Answers queries containing `needle` with annotated CSV.
    pub fn on(self: &Arc<Self>, needle: &str, csv: &str) -> Arc<Self> {
        self.rules
            .lock()
            .unwrap()
            .push((needle.to_owned(), Ok(csv.to_owned())));
        Arc::clone(self)
    }

    /// Fails queries containing `needle`.
    pub fn fail(self: &Arc<Self>, needle: &str, error: InfluxError) -> Arc<Self> {
        self.rules
            .lock()
            .unwrap()
            .push((needle.to_owned(), Err(error)));
        Arc::clone(self)
    }

    /// Waits before every answer.
    pub fn slow(self: &Arc<Self>, delay: Duration) -> Arc<Self> {
        *self.delay.lock().unwrap() = Some(delay);
        Arc::clone(self)
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl InfluxQuery for FakeStore {
    fn query<'a>(&'a self, flux: &'a str) -> BoxFuture<'a, Result<Vec<Row>, InfluxError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self
            .rules
            .lock()
            .unwrap()
            .iter()
            .find(|(needle, _)| flux.contains(needle.as_str()))
            .map(|(_, answer)| answer.clone())
            .unwrap_or_else(|| Ok(String::new()));
        let delay = *self.delay.lock().unwrap();
        Box::pin(async move {
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            parse_annotated_csv(&answer?)
        })
    }
}

/// The router over `store`, at [`NOW`].
pub fn app(store: Arc<FakeStore>) -> Router {
    router(AppState {
        store,
        bucket: "feedwatch".to_owned(),
        clock: Arc::new(|| NOW),
    })
}

/// GETs `path` and returns the status and JSON body.
pub async fn get(app: Router, path: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

/// Builds the annotated CSV InfluxDB returns for `last()` rows, from
/// `(measurement, chain, feed_id, status, time, field, value)`. Values are quoted, so commas are safe.
pub fn csv(rows: &[(&str, &str, &str, &str, &str, &str, &str)]) -> String {
    let mut out = String::from(
        "#datatype,string,long,string,string,string,string,dateTime:RFC3339,string,string\r\n\
         ,result,table,_measurement,chain,feed_id,status,_time,_field,_value\r\n",
    );
    for (i, (m, chain, feed, status, time, field, value)) in rows.iter().enumerate() {
        out.push_str(&format!(
            ",_result,{i},{m},{chain},{feed},{status},{time},{field},\"{}\"\r\n",
            value.replace('"', "\"\"")
        ));
    }
    out
}

// --- Real InfluxDB 2.9.1 in Docker (tests marked #[ignore]; CI=1 runs them). ---

pub const TOKEN: &str = "feedwatch-test-token";

pub async fn influx() -> (ContainerAsync<GenericImage>, String) {
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

/// Writes line protocol (seconds precision), retrying while setup mode restarts influxd.
pub async fn seed(url: &str, lines: &str) {
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

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
