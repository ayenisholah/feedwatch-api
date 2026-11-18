//! Feedwatch API: read-only HTTP over the InfluxDB data the poller writes.
//!
//! Routes are mounted under `/api` (feedwatch-platform `docs/DATA_CONTRACT.md#api`).
//! Module map: `routes` builds the router, `influx` runs Flux and `queries` builds and reads it
//! (FR-FW-6), `error` maps failures to responses, `cache` holds 30 s responses, `config` reads the
//! environment.

pub mod cache;
pub mod config;
pub mod error;
pub mod influx;
pub mod queries;
pub mod routes;
