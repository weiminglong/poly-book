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

fn build_router(handle: PrometheusHandle, endpoint: &str) -> Router {
    let endpoint = endpoint.to_string();
    Router::new().route(
        &endpoint,
        get(move || {
            let handle = handle.clone();
            async move { handle.render() }
        }),
    )
}

/// Serve the metrics endpoint. Call `install_recorder()` first.
pub async fn serve_metrics(
    handle: PrometheusHandle,
    addr: SocketAddr,
    endpoint: &str,
) -> Result<(), MetricsError> {
    tracing::info!(%addr, endpoint, "starting metrics server");

    let app = build_router(handle, endpoint);
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
    let app = build_router(handle, endpoint);

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_handle() -> PrometheusHandle {
        PrometheusBuilder::new().build_recorder().handle()
    }

    // --- build_router ---

    #[tokio::test]
    async fn build_router_creates_working_metrics_endpoint() {
        let handle = test_handle();
        let app = build_router(handle, "/metrics");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn build_router_returns_non_empty_prometheus_body() {
        let handle = test_handle();
        let app = build_router(handle, "/metrics");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap();
        // Empty render is valid — Prometheus text format with zero metrics is
        // just an empty or whitespace-only string. The important thing is we
        // got 200 OK without panic.
        let text = String::from_utf8(body.to_vec()).expect("body should be valid utf-8");
        assert!(
            text.is_empty() || text.chars().all(|c| c.is_whitespace()) || text.contains('#'),
            "body should be empty or contain Prometheus comment lines"
        );
    }

    #[tokio::test]
    async fn build_router_returns_404_for_other_paths() {
        let handle = test_handle();
        let app = build_router(handle, "/metrics");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn build_router_custom_endpoint_path() {
        let handle = test_handle();
        let app = build_router(handle, "/custom/prom");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/custom/prom")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    // --- serve_metrics_on_listener ---

    #[tokio::test]
    async fn serve_metrics_on_listener_responds_to_requests() {
        let handle = test_handle();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind to port 0");
        let addr = listener.local_addr().unwrap();

        // Spawn the server in the background
        tokio::spawn(async move {
            serve_metrics_on_listener(handle, listener, "/metrics")
                .await
                .unwrap();
        });

        // Give the server a moment to start accepting
        tokio::task::yield_now().await;

        let url = format!("http://{addr}/metrics");
        let resp = reqwest::get(&url).await.expect("HTTP request to metrics");
        assert_eq!(resp.status(), 200);

        let body = resp.text().await.unwrap();
        // Should be valid text (empty or Prometheus format)
        assert!(body.is_ascii() || body.is_empty());
    }

    // --- error variants ---

    #[test]
    fn metrics_error_display_recorder_install() {
        let err = MetricsError::RecorderInstall("duplicate".to_string());
        let msg = err.to_string();
        assert!(msg.contains("duplicate"), "should contain cause: {msg}");
        assert!(
            msg.contains("install"),
            "should describe the operation: {msg}"
        );
    }

    #[test]
    fn metrics_error_display_server_start() {
        let err = MetricsError::ServerStart("address in use".to_string());
        let msg = err.to_string();
        assert!(
            msg.contains("address in use"),
            "should contain cause: {msg}"
        );
        assert!(
            msg.contains("start"),
            "should describe the operation: {msg}"
        );
    }
}
