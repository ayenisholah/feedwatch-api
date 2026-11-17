//! Entry point. Wires modules together only (feedwatch-platform `docs/ENGINEERING_STANDARDS.md`).

use feedwatch_api::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.addr).await?;
    tracing::info!(addr = %config.addr, influx = %config.influx.url, "listening");
    axum::serve(listener, feedwatch_api::routes::router())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
