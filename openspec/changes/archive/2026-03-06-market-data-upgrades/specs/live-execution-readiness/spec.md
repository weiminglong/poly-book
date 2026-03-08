# Spec: Live Execution Readiness

## Execution Journal

### Scenario: Local order lifecycle is persisted

```
Given the strategy submits, cancels, or replaces an order
When the local trading system changes execution state
Then an `execution_event` is appended for each state transition
And order intent is not inferred from public market-data records
```

### Scenario: Exchange acknowledgements and fills are persisted separately

```
Given the exchange acknowledges, rejects, partially fills, or fully fills an order
When the execution update is received
Then the update is written to `execution_events`
And execution replay can reconstruct the strategy's own state independently of market-data replay
```

## Latency Telemetry

### Scenario: End-to-end timestamps are recorded for each order path

```
Given a strategy reacts to market data and submits an order
When the order progresses through the system
Then timestamps are persisted for market-data receive, normalization, strategy decision, order submit, acknowledgement, and fill
And the resulting latency breakdown is queryable for live diagnostics and simulation
```

## Storage Boundaries

### Scenario: Public market data and execution state remain separate

```
Given market-data storage and execution storage are both enabled
When data is persisted
Then public market-data datasets remain separate from `execution_events`
And a replay consumer can choose market-only, execution-only, or combined analysis explicitly
```
