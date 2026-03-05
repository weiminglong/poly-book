# Spec: Replay Engine

## Book Reconstruction

### Scenario: Reconstruct book at a timestamp with available snapshot

```
Given stored events contain a Snapshot at T=1000 and Deltas at T=1001, T=1002
When reconstruct_at is called with target_timestamp_us=1002
Then the engine reads events from [T-lookback, T=1002]
And finds the Snapshot at T=1000
And applies apply_snapshot with all bid/ask entries from that snapshot
And applies each Delta event with T <= 1002
And returns the reconstructed L2Book
```

### Scenario: No snapshot within lookback window

```
Given no Snapshot events exist in the lookback window before T=5000
When reconstruct_at is called with target_timestamp_us=5000
Then the engine returns ReplayError::NoSnapshotFound with the asset_id and timestamp
```

### Scenario: Multiple snapshot events at the same timestamp

```
Given a snapshot at T=1000 consists of 5 bid events and 3 ask events all with T=1000
When reconstruct_at is called with target_timestamp_us=1000
Then all 5 bid and 3 ask snapshot events are collected
And apply_snapshot is called with the aggregated bids and asks arrays
```

## Snapshot Finding

### Scenario: Most recent snapshot is selected

```
Given snapshots exist at T=500, T=800, and T=900 within the lookback window
When reconstruct_at targets T=1000
Then the snapshot at T=900 is selected (last matching via rposition)
```

### Scenario: Deltas before snapshot are ignored

```
Given a Delta at T=700 and a Snapshot at T=800 and a Delta at T=900
When reconstruct_at targets T=1000
Then only the Delta at T=900 is applied after the snapshot
And the Delta at T=700 is skipped
```

## Delta Application

### Scenario: Deltas without a side are skipped

```
Given a Snapshot at T=100 and a Delta at T=200 with side=None
When reconstruct_at targets T=300
Then the side-less Delta is not applied to the book
```

### Scenario: Deltas after target timestamp are excluded

```
Given a Snapshot at T=100 and Deltas at T=200, T=300, T=400
When reconstruct_at targets T=300
Then only Deltas at T=200 and T=300 are applied
And the Delta at T=400 is not applied
```

## Range Replay

### Scenario: Read all events in a time range

```
Given events exist at T=100, T=200, T=300
When replay_range is called with start_us=100, end_us=300
Then all three events are returned ordered by recv_timestamp_us
```

## EventReader: ParquetReader

### Scenario: Read events from time-partitioned Parquet files

```
Given Parquet files exist at data/2025/01/15/10/events_001.parquet
And the file contains events for asset "TOKEN_A" with timestamps in that hour
When read_events is called with asset_id="TOKEN_A" and a range covering that hour
Then events matching the asset_id and time range are returned
And events are sorted by recv_timestamp_us
```

### Scenario: Missing hourly directory is skipped

```
Given no directory exists for data/2025/01/15/11/
When read_events covers a range spanning hours 10 and 11
Then hour 10 events are returned
And no error is raised for the missing hour 11 directory
```

## EventReader: ClickHouseReader

### Scenario: Query events from ClickHouse

```
Given the orderbook_events table contains events for asset "TOKEN_B"
When read_events is called for "TOKEN_B" with a time range
Then a SQL query filters by asset_id and timestamp range
And results are returned ordered by recv_timestamp_us
```

## Backfill Loop

### Scenario: Periodic REST snapshot fetching

```
Given BackfillConfig with rest_url, token_ids=["T1","T2"], interval=60s
When run_backfill starts
Then it fetches GET /book?token_id=T1 and GET /book?token_id=T2
And converts bid/ask entries to OrderbookEvent with EventType::Snapshot
And sends events through the mpsc channel
And waits rate_limit_pause between requests
And waits interval between full cycles
```

### Scenario: Channel closed stops backfill

```
Given the mpsc receiver is dropped
When run_backfill attempts to send an event
Then it logs "backfill channel closed" and returns Ok(())
```

### Scenario: HTTP error is logged and skipped

```
Given the REST API returns HTTP 500 for token "T1"
When run_backfill processes token "T1"
Then it logs the error
And continues to the next token without crashing
```
