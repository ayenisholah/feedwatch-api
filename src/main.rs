//! Entry point. Wires modules together only (feedwatch-platform `docs/ENGINEERING_STANDARDS.md`).

use std::sync::Arc;
use std::time::Duration;

use feedwatch_api::config::Config;
use feedwatch_api::influx::HttpInflux;
use feedwatch_api::routes::{AppState, router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env()?;
    let bucket = config.influx.bucket.clone();
    let store = HttpInflux::new(config.influx.clone(), Duration::from_secs(10))?;
    let state = AppState::new(Arc::new(store), bucket);

    let listener = tokio::net::TcpListener::bind(config.addr).await?;
    tracing::info!(addr = %config.addr, influx = %config.influx.url, "listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
