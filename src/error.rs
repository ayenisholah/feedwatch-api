//! Errors a route can return, as JSON `{ "error": "…" }` with a fitting status.
//!
//! Part of FR-FW-6. InfluxDB failures become 503 with a generic message; the detail goes to the log.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::influx::InfluxError;

/// Why a request failed.
#[derive(Debug, PartialEq, Eq)]
pub enum ApiError {
    /// The request can't be answered as written, such as an unknown chain.
    BadRequest(String),
    /// Nothing exists at this path.
    NotFound(String),
    /// InfluxDB failed or timed out.
    Unavailable(InfluxError),
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
