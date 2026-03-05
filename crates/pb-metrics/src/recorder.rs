use metrics::{counter, describe_counter, describe_histogram, histogram};

/// Register all metric descriptions. Call once at startup.
pub fn register_metrics() {
    describe_counter!(
        "pb_messages_received_total",
        "Total WebSocket messages received"
    );
    describe_counter!("pb_snapshots_applied_total", "Total book snapshots applied");
    describe_counter!("pb_deltas_applied_total", "Total book deltas applied");
    describe_counter!("pb_trades_received_total", "Total trades received");
    describe_counter!("pb_gaps_detected_total", "Total sequence gaps detected");
    describe_counter!("pb_reconnections_total", "Total WebSocket reconnections");
    describe_counter!(
        "pb_storage_flushes_total",
        "Total storage flushes (parquet + clickhouse)"
    );
    describe_counter!("pb_rest_requests_total", "Total REST API requests made");

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
}

pub fn record_message_received(event_type: &str) {
    counter!("pb_messages_received_total", "event_type" => event_type.to_string()).increment(1);
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

pub fn record_reconnection() {
    counter!("pb_reconnections_total").increment(1);
}

pub fn record_storage_flush(sink_type: &str) {
    counter!("pb_storage_flushes_total", "sink" => sink_type.to_string()).increment(1);
}

pub fn record_rest_request() {
    counter!("pb_rest_requests_total").increment(1);
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
