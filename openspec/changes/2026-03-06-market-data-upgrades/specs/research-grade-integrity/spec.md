# Spec: Research-Grade Integrity

## Ingest Lifecycle Persistence

### Scenario: Sequence gaps are persisted as lifecycle events

```
Given the feed detects a missing sequence in an asset's event stream
When the gap is observed during ingestion or replay
Then an `ingest_event` is persisted describing the asset, timestamps, and gap
And the condition is available to downstream readers without requiring log access
```

### Scenario: Reconnects are recorded explicitly

```
Given the WebSocket client disconnects and reconnects
When the connection lifecycle changes state
Then reconnect start and reconnect success events are persisted
And replay consumers can identify continuity boundaries in the market-data stream
```

### Scenario: Stale snapshots are persisted when skipped

```
Given a snapshot arrives with an exchange timestamp older than the latest accepted snapshot
When the dispatcher skips the stale snapshot
Then the skip decision is persisted as an `ingest_event`
And the skipped snapshot does not silently disappear from audit trails
```

## Dataset Separation

### Scenario: Book events are stored separately from trade events

```
Given the feed emits snapshot, delta, and trade-related updates
When the storage layer persists them
Then snapshot and delta rows are written to `book_events`
And trade-related rows are written to `trade_events`
And downstream readers do not infer event meaning from overloaded generic rows
```

### Scenario: Dual-clock provenance is preserved

```
Given a normalized market-data event
When it is persisted
Then both `recv_timestamp_us` and `exchange_timestamp_us` are stored
And the persisted record includes source provenance needed to explain event ordering
```

## Reader Semantics

### Scenario: Readers surface continuity metadata

```
Given a replay request spans one or more reconnects or gap markers
When events are read from storage
Then continuity metadata is returned or queryable alongside the event stream
And the consumer can distinguish complete and best-effort replays
```
