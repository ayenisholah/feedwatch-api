//! The HTTP router. Only `/api/health` exists so far; FR-FW-6 adds the data routes in M2.

use axum::{Json, Router, routing::get};
use serde::Serialize;

/// Body of `GET /api/health`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Health {
    /// `ok` while the process is serving.
    pub status: &'static str,
}

/// Builds the application router.
pub fn router() -> Router {
    Router::new().route("/api/health", get(health))
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}
