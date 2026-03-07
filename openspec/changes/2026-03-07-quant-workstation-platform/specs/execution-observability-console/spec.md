# Spec: Execution Observability Console

## Order Lifecycle

### Scenario: Order timelines are queryable from execution storage

```
Given execution events exist for one or more local orders
When the workstation requests execution timelines for an asset, order, or time window
Then it receives the ordered lifecycle of those execution events
And the result is sourced from `execution_events` rather than public market-data records
```

### Scenario: Distinct exchange transitions remain visible

```
Given an order receives acknowledgement, reject, partial fill, fill, or terminal updates
When the workstation renders the order timeline
Then each update is represented as a separate execution state transition
And the consumer does not infer order state from aggregated status only
```

## Latency Traces

### Scenario: Per-order latency breakdowns are visible

```
Given execution events include latency trace fields
When the workstation requests execution diagnostics
Then the response exposes timestamps for market-data receive, normalization, strategy decision, order submit, acknowledgement, and fill when present
And the operator can inspect end-to-end latency attribution for that order path
```

## Storage Boundaries

### Scenario: Execution state remains separate from market-data state

```
Given market-data and execution storage are both enabled
When the workstation requests execution timelines
Then the execution response is built from execution-state records
And market-data truth and execution truth remain queryable as separate domains
```
