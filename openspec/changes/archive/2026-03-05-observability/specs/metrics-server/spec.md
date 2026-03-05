# Spec: Metrics Server

## Metric Registration

### Scenario: All metrics are described at startup

```
Given the application starts the ingest command with --metrics enabled
When register_metrics() is called
Then 8 counters are described with human-readable descriptions
And 3 histograms are described with human-readable descriptions
And all metrics are available for recording
```

## HTTP Endpoint

### Scenario: Prometheus scrape endpoint serves metrics

```
Given the metrics server is started on 0.0.0.0:9090 with endpoint /metrics
When a GET request is made to http://localhost:9090/metrics
Then the response contains Prometheus text format output
And all registered metrics appear in the response
```

### Scenario: Custom listen address and endpoint

```
Given metrics.listen_addr is set to "127.0.0.1:8080" in config
And metrics.endpoint is set to "/prom"
When the metrics server starts
Then it binds to 127.0.0.1:8080
And serves metrics at /prom
```

### Scenario: Server start failure

```
Given the configured listen address is already in use
When start_metrics_server attempts to bind
Then it returns MetricsError::ServerStart with the bind error message
```

## Counter Recording

### Scenario: Message counter increments with event type label

```
Given the Prometheus recorder is installed
When record_message_received("Delta") is called
Then pb_messages_received_total{event_type="Delta"} increments by 1
```

### Scenario: Storage flush counter with sink label

```
Given the Prometheus recorder is installed
When record_storage_flush("parquet") is called
Then pb_storage_flushes_total{sink="parquet"} increments by 1
```

### Scenario: Unlabeled counters increment

```
Given the Prometheus recorder is installed
When record_snapshot_applied() is called
Then pb_snapshots_applied_total increments by 1
```

### Scenario: Gap detection counter

```
Given the Prometheus recorder is installed
When record_gap_detected() is called
Then pb_gaps_detected_total increments by 1
```

### Scenario: Reconnection counter

```
Given the Prometheus recorder is installed
When record_reconnection() is called
Then pb_reconnections_total increments by 1
```

## Histogram Recording

### Scenario: Processing duration histogram

```
Given the Prometheus recorder is installed
When record_processing_duration_us(150.0) is called
Then pb_message_processing_duration_us records the value 150.0
```

### Scenario: Flush duration histogram

```
Given the Prometheus recorder is installed
When record_flush_duration_ms(45.5) is called
Then pb_storage_flush_duration_ms records the value 45.5
```

### Scenario: WebSocket latency histogram

```
Given the Prometheus recorder is installed
When record_ws_latency_us(320.0) is called
Then pb_ws_latency_us records the value 320.0
```

## Recorder Installation

### Scenario: Prometheus recorder installed globally

```
Given no global metrics recorder is installed
When start_metrics_server is called
Then PrometheusBuilder installs the recorder globally
And the recorder handle is used for rendering in the HTTP handler
```

### Scenario: Recorder installation failure

```
Given a global metrics recorder is already installed
When start_metrics_server attempts to install a second recorder
Then it returns MetricsError::RecorderInstall with the error message
```
