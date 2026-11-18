//! Errors a route can return, as JSON `{ "error": "…" }` with a fitting status.
//!
//! Part of FR-FW-6 and GR-2. InfluxDB failures become 503 with a generic message; the detail goes to
//! the log. Rate-limited requests get 429 with `Retry-After`.

use axum::Json;
use axum::http::StatusCode;
use axum::http::header::RETRY_AFTER;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use crate::influx::InfluxError;

/// The body of every error response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// What went wrong, for people.
    pub error: String,
}

/// Why a request failed.
#[derive(Debug, PartialEq, Eq)]
pub enum ApiError {
    /// The request can't be answered as written, such as an unknown chain.
    BadRequest(String),
    /// Nothing exists at this path.
    NotFound(String),
    /// InfluxDB failed or timed out.
    Unavailable(InfluxError),
    /// The client sent too many requests (GR-2).
    TooManyRequests,
}

impl From<InfluxError> for ApiError {
    fn from(error: InfluxError) -> ApiError {
        ApiError::Unavailable(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            ApiError::TooManyRequests => {
                let body = Json(json!({ "error": "too many requests; slow down" }));
                return (StatusCode::TOO_MANY_REQUESTS, [(RETRY_AFTER, "1")], body).into_response();
            }
            ApiError::Unavailable(error) => {
                tracing::error!(%error, "influxdb query failed");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "feed data is temporarily unavailable".to_owned(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
