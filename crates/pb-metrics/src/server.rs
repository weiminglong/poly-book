use axum::{routing::get, Router};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::net::SocketAddr;

use crate::error::MetricsError;

/// Install the Prometheus metrics recorder globally.
/// Must be called before `register_metrics()` or any `record_*` functions.
pub fn install_recorder() -> Result<PrometheusHandle, MetricsError> {
    PrometheusBuilder::new()
        .install_recorder()
        .map_err(|e| MetricsError::RecorderInstall(e.to_string()))
}

/// Serve the metrics endpoint. Call `install_recorder()` first.
pub async fn serve_metrics(
    handle: PrometheusHandle,
    addr: SocketAddr,
    endpoint: &str,
) -> Result<(), MetricsError> {
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

/// Serve metrics on a pre-bound listener.
/// Use this when you want to bind the listener yourself (e.g., to catch bind errors early).
pub async fn serve_metrics_on_listener(
    handle: PrometheusHandle,
    listener: tokio::net::TcpListener,
    endpoint: &str,
) -> Result<(), MetricsError> {
    let endpoint = endpoint.to_string();
    let app = Router::new().route(
        &endpoint,
        get(move || {
            let handle = handle.clone();
            async move { handle.render() }
        }),
    );

    axum::serve(listener, app)
        .await
        .map_err(|e| MetricsError::ServerStart(e.to_string()))?;

    Ok(())
}

/// Start the Prometheus metrics HTTP server (legacy convenience function).
///
/// Installs the Prometheus recorder globally and serves metrics at the given
/// address and endpoint path.
pub async fn start_metrics_server(addr: SocketAddr, endpoint: &str) -> Result<(), MetricsError> {
    let handle = install_recorder()?;
    serve_metrics(handle, addr, endpoint).await
}
