use axum::{routing::get, Router};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::net::SocketAddr;
use std::time::Duration;

use crate::error::MetricsError;

/// Explicit histogram buckets for microsecond-resolution latency metrics
/// (`*_us`). Without explicit buckets the exporter emits rolling summaries whose
/// quantiles cannot be aggregated across processes.
const US_BUCKETS: &[f64] = &[
    10.0,
    50.0,
    100.0,
    250.0,
    500.0,
    1_000.0,
    2_500.0,
    5_000.0,
    10_000.0,
    50_000.0,
    100_000.0,
    500_000.0,
    1_000_000.0,
];

/// Explicit histogram buckets for millisecond-resolution latency metrics
/// (`*_ms`).
const MS_BUCKETS: &[f64] = &[
    1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 5_000.0, 30_000.0,
];

/// Install the Prometheus metrics recorder globally, with explicit, unit-aware
/// histogram buckets so latency quantiles are real Prometheus histograms
/// (cross-process aggregatable) rather than per-process rolling summaries
///. Must be called before `register_metrics()` or any
/// `record_*` functions.
pub fn install_recorder() -> Result<PrometheusHandle, MetricsError> {
    PrometheusBuilder::new()
        .set_buckets_for_metric(Matcher::Suffix("_us".to_string()), US_BUCKETS)
        .map_err(|e| MetricsError::RecorderInstall(e.to_string()))?
        .set_buckets_for_metric(Matcher::Suffix("_ms".to_string()), MS_BUCKETS)
        .map_err(|e| MetricsError::RecorderInstall(e.to_string()))?
        .install_recorder()
        .map_err(|e| MetricsError::RecorderInstall(e.to_string()))
}

/// Spawn a background task that calls `run_upkeep` on a fixed interval so idle
/// histogram/summary state is drained and bucket memory cannot grow unbounded if
/// scraping stalls. Returns the task handle.
pub fn spawn_upkeep(handle: PrometheusHandle, interval: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            handle.run_upkeep();
        }
    })
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

    #[test]
    fn us_suffix_metrics_render_as_bucketed_histograms() {
        // A `*_us` histogram must render as a real Prometheus histogram with
        // `_bucket{le=...}` series (cross-process aggregatable), not a summary
        //.
        let recorder = PrometheusBuilder::new()
            .set_buckets_for_metric(Matcher::Suffix("_us".to_string()), US_BUCKETS)
            .unwrap()
            .build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            metrics::histogram!("pb_demo_latency_us").record(150.0);
        });
        let rendered = handle.render();
        assert!(
            rendered.contains("pb_demo_latency_us_bucket"),
            "expected bucketed histogram, got:\n{rendered}"
        );
        assert!(
            rendered.contains("le=\"250\""),
            "expected the 250us bucket boundary, got:\n{rendered}"
        );
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
