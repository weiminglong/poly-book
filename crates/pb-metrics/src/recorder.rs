use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};

/// Register all metric descriptions. Call once at startup.
pub fn register_metrics() {
    describe_counter!(
        "pb_messages_received_total",
        "Total WebSocket messages received"
    );
    describe_counter!(
        "pb_unknown_messages_dropped_total",
        "Total WebSocket frames dropped because they did not deserialize into a known message type"
    );
    describe_counter!(
        "pb_book_mismatches_total",
        "Total times the reconstructed top-of-book diverged from the venue-stated best bid/ask"
    );
    describe_counter!(
        "pb_clock_skew_events_total",
        "Total messages whose venue timestamp was implausibly ahead of receive time (clock skew)"
    );
    describe_histogram!(
        "pb_clock_skew_us",
        "Observed venue-ahead-of-receive clock skew in microseconds"
    );
    describe_histogram!(
        "pb_recv_to_durable_us",
        "End-to-end latency from feed receive to durable WAL append, in microseconds"
    );
    describe_counter!("pb_snapshots_applied_total", "Total book snapshots applied");
    describe_counter!("pb_deltas_applied_total", "Total book deltas applied");
    describe_counter!("pb_trades_received_total", "Total trades received");
    describe_counter!("pb_gaps_detected_total", "Total sequence gaps detected");
    describe_counter!(
        "pb_crossed_books_total",
        "Total times the live book was detected crossed/locked (best_bid >= best_ask)"
    );
    describe_counter!("pb_reconnections_total", "Total WebSocket reconnections");
    describe_counter!(
        "pb_snapshots_reconciled_total",
        "Total snapshots that passed staleness check"
    );
    describe_counter!(
        "pb_stale_snapshots_skipped_total",
        "Total stale snapshots skipped"
    );
    describe_counter!(
        "pb_storage_flushes_total",
        "Total storage flushes (parquet + clickhouse)"
    );
    describe_counter!("pb_rest_requests_total", "Total REST API requests made");
    describe_counter!("pb_rotations_total", "Total market rotations performed");
    describe_counter!(
        "pb_discovery_failures_total",
        "Total failed market discovery attempts"
    );
    describe_counter!(
        "pb_wal_append_failures_total",
        "Total WAL append failures (each is a durability-fatal event)"
    );
    describe_counter!(
        "pb_sink_flush_failures_total",
        "Total storage sink flush failures (per sink), including retries"
    );

    // --- Gauges: on-call needs current state, not just cumulative counters ---
    describe_gauge!(
        "pb_wal_consumer_lag_bytes",
        "Current byte lag of the serve WAL reader behind the writer"
    );
    describe_gauge!(
        "pb_feed_staleness_seconds",
        "Seconds since the most recent message was received from the feed"
    );

    describe_histogram!(
        "pb_message_processing_duration_us",
        "Time to process a single message (microseconds)"
    );
    describe_histogram!(
        "pb_storage_flush_duration_ms",
        "Time to flush to storage (milliseconds)"
    );
    describe_histogram!(
        "pb_ws_latency_us",
        "WebSocket message latency (recv - exchange timestamp, microseconds)"
    );
    describe_histogram!(
        "pb_api_request_duration_ms",
        "Time to serve an API request (milliseconds)"
    );
}

pub fn record_message_received(event_type: &'static str) {
    counter!("pb_messages_received_total", "event_type" => event_type).increment(1);
}

/// A WebSocket frame that did not deserialize into any known message type was
/// dropped. Previously silent (debug-only), so genuine silent loss of new/unknown
/// venue message types was invisible to operators (audit finding A.110).
pub fn record_unknown_message_dropped() {
    counter!("pb_unknown_messages_dropped_total").increment(1);
}

/// The reconstructed top-of-book diverged from the venue-stated best_bid/best_ask
/// after applying a delta — a silently dropped/corrupt update (A.74/A.109).
pub fn record_book_mismatch() {
    counter!("pb_book_mismatches_total").increment(1);
}

/// A message's venue (exchange) timestamp was implausibly ahead of our receive
/// timestamp — evidence of clock skew between our host and the venue, which would
/// corrupt exchange-time replay ordering (audit finding A.76). The observed skew
/// (µs) is also recorded as a histogram for distribution analysis.
pub fn record_clock_skew(skew_us: u64) {
    counter!("pb_clock_skew_events_total").increment(1);
    histogram!("pb_clock_skew_us").record(skew_us as f64);
}

/// End-to-end latency (µs) from feed receive to durable WAL append, the headline
/// ingest-path latency the audit found missing (A.113). Bucketed via the `_us`
/// histogram-bucket policy.
pub fn record_recv_to_durable_us(latency_us: u64) {
    histogram!("pb_recv_to_durable_us").record(latency_us as f64);
}

pub fn record_snapshot_applied() {
    counter!("pb_snapshots_applied_total").increment(1);
}

pub fn record_delta_applied() {
    counter!("pb_deltas_applied_total").increment(1);
}

pub fn record_trade_received() {
    counter!("pb_trades_received_total").increment(1);
}

pub fn record_gap_detected() {
    counter!("pb_gaps_detected_total").increment(1);
}

pub fn record_crossed_book() {
    counter!("pb_crossed_books_total").increment(1);
}

pub fn record_wal_append_failure() {
    counter!("pb_wal_append_failures_total").increment(1);
}

pub fn record_sink_flush_failure(sink: &'static str) {
    counter!("pb_sink_flush_failures_total", "sink" => sink).increment(1);
}

/// Set the current WAL consumer lag (bytes the serve reader is behind).
pub fn set_wal_consumer_lag_bytes(lag: u64) {
    gauge!("pb_wal_consumer_lag_bytes").set(lag as f64);
}

/// Set the current feed staleness (seconds since the last received message).
pub fn set_feed_staleness_seconds(secs: f64) {
    gauge!("pb_feed_staleness_seconds").set(secs);
}

pub fn record_reconnection() {
    counter!("pb_reconnections_total").increment(1);
}

pub fn record_storage_flush(sink_type: &'static str) {
    counter!("pb_storage_flushes_total", "sink" => sink_type).increment(1);
}

pub fn record_rest_request() {
    counter!("pb_rest_requests_total").increment(1);
}

pub fn record_snapshot_reconciled() {
    counter!("pb_snapshots_reconciled_total").increment(1);
}

pub fn record_stale_snapshot_skipped() {
    counter!("pb_stale_snapshots_skipped_total").increment(1);
}

pub fn record_processing_duration_us(duration_us: f64) {
    histogram!("pb_message_processing_duration_us").record(duration_us);
}

pub fn record_flush_duration_ms(duration_ms: f64) {
    histogram!("pb_storage_flush_duration_ms").record(duration_ms);
}

pub fn record_ws_latency_us(latency_us: f64) {
    histogram!("pb_ws_latency_us").record(latency_us);
}

pub fn record_api_request_duration_ms(method: &str, route: &str, status: u16, duration_ms: f64) {
    histogram!(
        "pb_api_request_duration_ms",
        "method" => method.to_string(),
        "route" => route.to_string(),
        "status" => status_to_static(status)
    )
    .record(duration_ms);
}

/// Map common HTTP status codes to static string slices to avoid per-call
/// `u16` formatting allocations on the hot path.
fn status_to_static(code: u16) -> &'static str {
    match code {
        200 => "200",
        201 => "201",
        204 => "204",
        304 => "304",
        400 => "400",
        404 => "404",
        405 => "405",
        422 => "422",
        429 => "429",
        500 => "500",
        502 => "502",
        503 => "503",
        _ => "other",
    }
}

pub fn record_rotation() {
    counter!("pb_rotations_total").increment(1);
}

pub fn record_discovery_failure() {
    counter!("pb_discovery_failures_total").increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- status_to_static ---

    #[test]
    fn status_to_static_maps_common_2xx_codes() {
        assert_eq!(status_to_static(200), "200");
        assert_eq!(status_to_static(201), "201");
        assert_eq!(status_to_static(204), "204");
    }

    #[test]
    fn status_to_static_maps_304() {
        assert_eq!(status_to_static(304), "304");
    }

    #[test]
    fn status_to_static_maps_client_error_codes() {
        assert_eq!(status_to_static(400), "400");
        assert_eq!(status_to_static(404), "404");
        assert_eq!(status_to_static(405), "405");
        assert_eq!(status_to_static(422), "422");
        assert_eq!(status_to_static(429), "429");
    }

    #[test]
    fn status_to_static_maps_server_error_codes() {
        assert_eq!(status_to_static(500), "500");
        assert_eq!(status_to_static(502), "502");
        assert_eq!(status_to_static(503), "503");
    }

    #[test]
    fn status_to_static_returns_other_for_unmapped_codes() {
        assert_eq!(status_to_static(100), "other");
        assert_eq!(status_to_static(101), "other");
        assert_eq!(status_to_static(202), "other");
        assert_eq!(status_to_static(301), "other");
        assert_eq!(status_to_static(302), "other");
        assert_eq!(status_to_static(401), "other");
        assert_eq!(status_to_static(403), "other");
        assert_eq!(status_to_static(504), "other");
        assert_eq!(status_to_static(0), "other");
        assert_eq!(status_to_static(999), "other");
    }

    #[test]
    fn status_to_static_returns_static_str() {
        // Verify the returned &str has 'static lifetime by binding to a variable
        // that outlives the match scope.
        let s: &'static str = status_to_static(200);
        assert!(!s.is_empty());
    }
}
