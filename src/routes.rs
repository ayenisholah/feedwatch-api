//! The HTTP router: `/api/health` (NFR-2), the feed, compare and incident routes (FR-FW-6),
//! `/api/openapi.json` ([`ApiDoc`]), and the public-API limits (GR-2): GET only, CORS for the site
//! origin, per-client rate limits and a 30 s cache.
//!
//! Handlers only parse the request, run a query from [`crate::queries`] through the injected
//! [`InfluxQuery`], and shape the result. Time is injected too, so tests control it.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::{IntoParams, OpenApi, ToSchema};

use crate::cache::{Cached, ResponseCache, TTL_S};
use crate::error::{ApiError, ErrorBody};
use crate::influx::InfluxQuery;
use crate::limits::{RateLimiter, client_ip};
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
    /// The site origin CORS allows (GR-2).
    pub allowed_origin: String,
    /// Per-client rate limits (GR-2).
    pub limiter: Arc<RateLimiter>,
    /// Cached responses of the data routes (GR-2).
    pub cache: Arc<ResponseCache>,
}

impl AppState {
    /// State with the system clock and empty limits and cache.
    pub fn new(store: Arc<dyn InfluxQuery>, bucket: String, allowed_origin: String) -> AppState {
        AppState::with_clock(
            store,
            bucket,
            allowed_origin,
            Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            }),
        )
    }

    /// State with the given clock, for tests.
    pub fn with_clock(
        store: Arc<dyn InfluxQuery>,
        bucket: String,
        allowed_origin: String,
        clock: Clock,
    ) -> AppState {
        AppState {
            store,
            bucket,
            clock,
            allowed_origin,
            limiter: Arc::new(RateLimiter::default()),
            cache: Arc::new(ResponseCache::default()),
        }
    }
}

/// Builds the application router.
///
/// Every route is GET only (other methods get 405), rate-limited per client, and allows CORS from
/// the site origin only. The data routes are also cached for [`TTL_S`] seconds (GR-2).
pub fn router(state: AppState) -> Router {
    let cached = Router::new()
        .route("/api/feeds", get(feeds))
        .route("/api/feeds/{chain}/{feed_id}", get(feed_detail))
        .route("/api/compare", get(compare_feeds))
        .route("/api/incidents", get(incidents))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            cache_responses,
        ));
    Router::new()
        .route("/api/health", get(health))
        .route("/api/openapi.json", get(openapi_json))
        .merge(cached)
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit))
        .layer(cors(&state.allowed_origin))
        .with_state(state)
}

fn cors(origin: &str) -> CorsLayer {
    // `list` sends Access-Control-Allow-Origin only when the request's Origin matches; an invalid
    // configured origin allows none.
    let allow = AllowOrigin::list(HeaderValue::from_str(origin).ok());
    CorsLayer::new()
        .allow_origin(allow)
        .allow_methods([Method::GET, Method::OPTIONS])
}

async fn rate_limit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if state.limiter.allow(client_ip(&request), (state.clock)()) {
        next.run(request).await
    } else {
        ApiError::TooManyRequests.into_response()
    }
}

/// Bodies larger than this aren't cached.
const MAX_CACHED_BODY: usize = 4 * 1024 * 1024;

async fn cache_responses(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let key = request
        .uri()
        .path_and_query()
        .map_or_else(String::new, |p| p.as_str().to_owned());
    let now = (state.clock)();
    if let Some(hit) = state.cache.get(&key, now) {
        let mut response = hit.body.into_response();
        if let Some(content_type) = hit.content_type {
            response.headers_mut().insert(CONTENT_TYPE, content_type);
        }
        response.headers_mut().insert(CACHE_CONTROL, max_age());
        return response;
    }
    let response = next.run(request).await;
    if response.status() != StatusCode::OK {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_CACHED_BODY).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    parts.headers.insert(CACHE_CONTROL, max_age());
    state.cache.put(
        key,
        Cached {
            stored_at: now,
            content_type: parts.headers.get(CONTENT_TYPE).cloned(),
            body: bytes.clone(),
        },
    );
    Response::from_parts(parts, bytes.into())
}

// The header below must say the same as the cache's lifetime.
const _: () = assert!(TTL_S == 30, "update the Cache-Control max-age");

fn max_age() -> HeaderValue {
    HeaderValue::from_static("public, max-age=30")
}

/// The OpenAPI document for every route, the contract feedwatch-web builds against
/// (feedwatch-platform `docs/DATA_CONTRACT.md`).
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Feedwatch API",
        description = "Read-only data on RedStone push feeds on Ethereum, Arbitrum and Base: freshness, missed updates, cross-chain spread and incidents."
    ),
    paths(health, feeds, feed_detail, compare_feeds, incidents),
    components(schemas(ErrorBody))
)]
pub struct ApiDoc;

async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// Body of `GET /api/health`.
#[derive(Debug, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// `ok`, or `degraded` when InfluxDB is down or the poller has stopped writing.
    pub status: &'static str,
    /// `ok` or `down`.
    pub influx: &'static str,
    /// When the poller last finished a cycle, Unix seconds.
    pub last_poll_at: Option<u64>,
}

/// Health of the API, InfluxDB and the poller. Always 200 while the API runs; answers within 1 s.
#[utoipa::path(get, path = "/api/health", tag = "health",
    responses((status = 200, description = "Health; `degraded` if InfluxDB is down or the last poll is over 180 s old", body = Health)))]
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

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct FeedsParams {
    /// Only this chain: `ethereum`, `arbitrum` or `base`.
    #[param(example = "base")]
    chain: Option<String>,
}

/// Body of `GET /api/feeds`.
#[derive(Debug, Serialize, PartialEq, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeedsResponse {
    /// When the poller last finished a cycle, Unix seconds.
    pub last_poll_at: Option<u64>,
    /// Every feed seen in the last 15 minutes, by chain then feed ID.
    pub feeds: Vec<FeedSummary>,
}

/// Every feed's latest round and check, seen in the last 15 minutes.
#[utoipa::path(get, path = "/api/feeds", tag = "feeds", params(FeedsParams),
    responses(
        (status = 200, description = "Feeds by chain, then feed ID", body = FeedsResponse),
        (status = 400, description = "Unknown chain", body = ErrorBody),
        (status = 503, description = "InfluxDB unavailable", body = ErrorBody)))]
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
#[derive(Debug, Serialize, PartialEq, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeedDetail {
    /// The latest round and check, if the feed was seen in the last 15 minutes.
    pub latest: Option<FeedSummary>,
    /// Value and age over 24 hours, in 5-minute windows, oldest first.
    pub series: Vec<SeriesPoint>,
}

/// One feed: its latest round and check, and 24 hours of value and age in 5-minute windows.
#[utoipa::path(get, path = "/api/feeds/{chain}/{feedId}", tag = "feeds",
    params(
        ("chain" = String, Path, description = "`ethereum`, `arbitrum` or `base`", example = "ethereum"),
        ("feedId" = String, Path, description = "The manifest key", example = "ETH")),
    responses(
        (status = 200, description = "The feed", body = FeedDetail),
        (status = 400, description = "Unknown chain", body = ErrorBody),
        (status = 404, description = "No data for this feed in 24 hours", body = ErrorBody),
        (status = 503, description = "InfluxDB unavailable", body = ErrorBody)))]
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
#[derive(Debug, Serialize, PartialEq, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompareResponse {
    /// When the poller last finished a cycle, Unix seconds.
    pub last_poll_at: Option<u64>,
    /// The feeds present on Ethereum, Arbitrum and Base, by feed ID.
    pub feeds: Vec<CompareRow>,
}

/// The feeds present on all three chains, side by side, with their spread.
#[utoipa::path(get, path = "/api/compare", tag = "feeds",
    responses(
        (status = 200, description = "Feeds on every chain, by feed ID", body = CompareResponse),
        (status = 503, description = "InfluxDB unavailable", body = ErrorBody)))]
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

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct IncidentsParams {
    /// `open` (the default) or `all`.
    #[param(example = "all")]
    status: Option<String>,
}

/// Body of `GET /api/incidents`.
#[derive(Debug, Serialize, PartialEq, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncidentsResponse {
    /// Incidents from the last 30 days, newest first, at most 500.
    pub incidents: Vec<Incident>,
}

/// Incidents from the last 30 days, newest first, at most 500.
#[utoipa::path(get, path = "/api/incidents", tag = "incidents", params(IncidentsParams),
    responses(
        (status = 200, description = "Incidents", body = IncidentsResponse),
        (status = 400, description = "Unknown status", body = ErrorBody),
        (status = 503, description = "InfluxDB unavailable", body = ErrorBody)))]
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
