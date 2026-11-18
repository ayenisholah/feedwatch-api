//! The HTTP router: `/api/health` (NFR-2), and the feed, compare and incident routes (FR-FW-6).
//!
//! Handlers only parse the request, run a query from [`crate::queries`] through the injected
//! [`InfluxQuery`], and shape the result. Time is injected too, so tests control it.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::influx::InfluxQuery;
use crate::queries::{
    Chain, CompareRow, FeedSummary, Incident, SeriesPoint, compare, health_query, incidents_query,
    last_poll_at, latest_by_feed, latest_query, pair_incidents, series_points, series_query,
    valid_feed_id,
};

/// How long `/api/health` waits for InfluxDB, so it answers within 1 s (NFR-2).
pub const HEALTH_TIMEOUT: Duration = Duration::from_millis(800);
/// A last poll older than this makes health `degraded`: three missed 60 s cycles.
pub const POLL_STALE_AFTER_S: u64 = 180;
/// How far back the latest-value queries look.
const LATEST_WINDOW_S: u64 = 900;

/// Returns the current Unix time in seconds.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// What every handler shares.
#[derive(Clone)]
pub struct AppState {
    /// Runs Flux queries.
    pub store: Arc<dyn InfluxQuery>,
    /// The bucket the poller writes to.
    pub bucket: String,
    /// The current time.
    pub clock: Clock,
}

impl AppState {
    /// State with the system clock.
    pub fn new(store: Arc<dyn InfluxQuery>, bucket: String) -> AppState {
        AppState {
            store,
            bucket,
            clock: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            }),
        }
    }
}

/// Builds the application router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/feeds", get(feeds))
        .route("/api/feeds/{chain}/{feed_id}", get(feed_detail))
        .route("/api/compare", get(compare_feeds))
        .route("/api/incidents", get(incidents))
        .with_state(state)
}

/// Body of `GET /api/health`.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// `ok`, or `degraded` when InfluxDB is down or the poller has stopped writing.
    pub status: &'static str,
    /// `ok` or `down`.
    pub influx: &'static str,
    /// When the poller last finished a cycle, Unix seconds.
    pub last_poll_at: Option<u64>,
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    let query = health_query(&state.bucket);
    let rows = tokio::time::timeout(HEALTH_TIMEOUT, state.store.query(&query)).await;
    let (influx, last_poll_at) = match rows {
        Ok(Ok(rows)) => ("ok", last_poll_at(&rows)),
        Ok(Err(error)) => {
            tracing::warn!(%error, "health: influxdb query failed");
            ("down", None)
        }
        Err(_) => {
            tracing::warn!("health: influxdb timed out");
            ("down", None)
        }
    };
    let fresh =
        last_poll_at.is_some_and(|t| (state.clock)().saturating_sub(t) <= POLL_STALE_AFTER_S);
    Json(Health {
        status: if influx == "ok" && fresh {
            "ok"
        } else {
            "degraded"
        },
        influx,
        last_poll_at,
    })
}

#[derive(Debug, Deserialize)]
struct FeedsParams {
    chain: Option<String>,
}

/// Body of `GET /api/feeds`.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedsResponse {
    /// When the poller last finished a cycle, Unix seconds.
    pub last_poll_at: Option<u64>,
    /// Every feed seen in the last 15 minutes, by chain then feed ID.
    pub feeds: Vec<FeedSummary>,
}

async fn feeds(
    State(state): State<AppState>,
    Query(params): Query<FeedsParams>,
) -> Result<Json<FeedsResponse>, ApiError> {
    let chain = match params.chain.as_deref().filter(|c| !c.is_empty()) {
        None => None,
        Some(text) => Some(parse_chain(text)?),
    };
    let rows = state
        .store
        .query(&latest_query(&state.bucket, chain, None, LATEST_WINDOW_S))
        .await?;
    Ok(Json(FeedsResponse {
        last_poll_at: last_poll_at(&rows),
        feeds: latest_by_feed(&rows),
    }))
}

/// Body of `GET /api/feeds/{chain}/{feedId}`.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedDetail {
    /// The latest round and check, if the feed was seen in the last 15 minutes.
    pub latest: Option<FeedSummary>,
    /// Value and age over 24 hours, in 5-minute windows, oldest first.
    pub series: Vec<SeriesPoint>,
}

async fn feed_detail(
    State(state): State<AppState>,
    Path((chain, feed_id)): Path<(String, String)>,
) -> Result<Json<FeedDetail>, ApiError> {
    let chain = parse_chain(&chain)?;
    if !valid_feed_id(&feed_id) {
        return Err(not_found(chain, &feed_id));
    }
    let latest_flux = latest_query(&state.bucket, Some(chain), Some(&feed_id), LATEST_WINDOW_S);
    let series_flux = series_query(&state.bucket, chain, &feed_id);
    let (latest, series) = tokio::join!(
        state.store.query(&latest_flux),
        state.store.query(&series_flux)
    );
    let latest = latest_by_feed(&latest?)
        .into_iter()
        .find(|f| f.chain == chain && f.feed_id == feed_id);
    let series = series_points(&series?);
    if latest.is_none() && series.is_empty() {
        return Err(not_found(chain, &feed_id));
    }
    Ok(Json(FeedDetail { latest, series }))
}

/// Body of `GET /api/compare`.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompareResponse {
    /// When the poller last finished a cycle, Unix seconds.
    pub last_poll_at: Option<u64>,
    /// The feeds present on Ethereum, Arbitrum and Base, by feed ID.
    pub feeds: Vec<CompareRow>,
}

async fn compare_feeds(State(state): State<AppState>) -> Result<Json<CompareResponse>, ApiError> {
    let rows = state
        .store
        .query(&latest_query(&state.bucket, None, None, LATEST_WINDOW_S))
        .await?;
    Ok(Json(CompareResponse {
        last_poll_at: last_poll_at(&rows),
        feeds: compare(&latest_by_feed(&rows)),
    }))
}

#[derive(Debug, Deserialize)]
struct IncidentsParams {
    status: Option<String>,
}

/// Body of `GET /api/incidents`.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IncidentsResponse {
    /// Incidents from the last 30 days, newest first, at most 500.
    pub incidents: Vec<Incident>,
}

async fn incidents(
    State(state): State<AppState>,
    Query(params): Query<IncidentsParams>,
) -> Result<Json<IncidentsResponse>, ApiError> {
    let open_only = match params.status.as_deref() {
        None | Some("" | "open") => true,
        Some("all") => false,
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "unknown status {other:?}; expected open or all"
            )));
        }
    };
    let rows = state.store.query(&incidents_query(&state.bucket)).await?;
    Ok(Json(IncidentsResponse {
        incidents: pair_incidents(&rows, open_only),
    }))
}

fn parse_chain(text: &str) -> Result<Chain, ApiError> {
    Chain::parse(text).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "unknown chain {text:?}; expected ethereum, arbitrum or base"
        ))
    })
}

fn not_found(chain: Chain, feed_id: &str) -> ApiError {
    let shown: String = feed_id.chars().take(64).collect();
    ApiError::NotFound(format!(
        "no data for feed {shown:?} on {} in the last 24 hours",
        chain.as_str()
    ))
}
