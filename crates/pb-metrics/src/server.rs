use axum::{routing::get, Router};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;

use crate::error::MetricsError;

/// Start the Prometheus metrics HTTP server.
///
/// Installs the Prometheus recorder globally and serves metrics at the given
/// address and endpoint path.
pub async fn start_metrics_server(addr: SocketAddr, endpoint: &str) -> Result<(), MetricsError> {
    let builder = PrometheusBuilder::new();
    let handle = builder
        .install_recorder()
        .map_err(|e| MetricsError::RecorderInstall(e.to_string()))?;

    let endpoint = endpoint.to_string();
    let app = Router::new().route(
        &endpoint,
        get(move || {
            let handle = handle.clone();
            async move { handle.render() }
        }),
    );

    tracing::info!(%addr, endpoint = endpoint.as_str(), "starting metrics server");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| MetricsError::ServerStart(e.to_string()))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| MetricsError::ServerStart(e.to_string()))?;

    Ok(())
}
