# poly-book On-Call Runbook

Operational responses for the alerts in [`alerts.yml`](alerts.yml). Each section
matches an alert name (anchor = lower-cased alert name). Metrics are exposed on
the `/metrics` endpoint (default `:9090`); health on `/health/live` and
`/health/ready`.

General triage order: **is the WAL still appending?** (durability) → **is the
feed live?** (capture) → **are sinks/serve keeping up?** (derived views). The WAL
is the source of truth: as long as it is appending and fsyncing, storage and the
read model can always be rebuilt.

---

## WalAppendFailing
**Severity: critical — active data loss.**

The WAL failed to append; `ingest` treats this as fatal and exits non-zero.

1. Check disk on the WAL volume (`wal.base_path`): `df -h`. Disk-full is the most
   common cause.
2. Check permissions/mount health on that volume.
3. If disk-full: confirm pruning is running (it prunes consumed segments on a 60s
   cadence) and that `wal.max_segments` / `wal.max_consumer_lag_bytes` are sized
   for the volume; a stuck/lagging consumer (see **WalConsumerLagHigh**) prevents
   pruning — resync it.
4. Free space or grow the volume, then restart `ingest`. Records buffered only in
   memory at the time of failure are lost; the gap is bounded by the
   flush/sync interval.

## SinkFlushFailing
**Severity: warning — storage falling behind; WAL still durable.**

A Parquet or ClickHouse flush failed (the sink retries with backoff and retains
its buffer; only after `MAX_FLUSH_RETRIES` does it surface the error).

1. Identify the sink from the `sink` label (`parquet` / `clickhouse`).
2. Parquet: check the object store (S3 creds/role, bucket policy, KMS key access),
   or local disk. ClickHouse: check the server is up and the schema matches.
3. The WAL still has every record. Once storage is healthy, rebuild any lost
   Parquet window with `cargo run -- reconcile` (offline — stop ingest first).

## FeedSilent
**Severity: critical — capture gap.**

No WS messages for 2 minutes.

1. Check the venue status and network egress from the ingest host.
2. Check ingest logs for reconnect attempts (the client reconnects with backoff +
   jitter and has a liveness watchdog). Confirm `/health/ready`.
3. If the venue is up but the client is stuck, restart `ingest`; on reconnect the
   dispatcher emits a `SourceReset` continuity marker and a fresh snapshot.

## FeedStale
**Severity: warning.**

`pb_feed_staleness_seconds` exceeded the threshold (last applied update is old).

1. During known-quiet periods this can be benign. During active market hours it
   means the feed stalled — see **FeedSilent**.
2. Cross-check `rate(pb_messages_received_total[1m])`.

## BookMismatch
**Severity: warning — silent feed corruption suspected.**

Our reconstructed top-of-book diverged from the venue-stated `best_bid`/`best_ask`
after a delta (`pb_book_mismatches_total`). A `BookMismatch` ingest event is
persisted for the affected asset, so the window is queryable.

1. Find the affected `asset_id` from the persisted ingest events
   (`/api/v1/integrity/summary` or the `ingest_events` dataset).
2. A resnapshot reseeds the shadow book: in `auto-ingest` this happens at the next
   market rotation; otherwise restart ingest or force a REST backfill snapshot.
3. If mismatches persist for one asset, suspect a venue-side delta we mis-apply
   (e.g. a new field/semantics) — investigate the raw frames.

## CrossedBook
**Severity: warning.**

Best bid ≥ best ask in the live or replayed book.

1. Cross-check with **BookMismatch** (same root cause class — a missed/misordered
   delta).
2. For replay, confirm the window is not straddling a `SourceReset`. For live,
   a resnapshot should clear a transient cross.

## SequenceGaps
**Severity: info.**

Expected briefly around reconnects (continuity reset). Investigate only if
sustained outside reconnect windows.

## ClockSkew
**Severity: warning.**

Venue (exchange) timestamps are arriving more than the tolerance ahead of our
receive time (`pb_clock_skew_events_total` / `pb_clock_skew_us`), i.e. the ingest
host clock is behind the venue's.

1. Check NTP/chrony on the ingest host (`chronyc tracking` / `timedatectl`);
   resync the clock.
2. Until corrected, prefer **RecvTime** replay (it uses our monotonic
   receive-ordering + ingest ordinal) over ExchangeTime, whose ordering depends
   on the venue clock.
3. Persistent skew with NTP healthy may indicate a venue-side timestamp anomaly —
   capture samples and compare against `pb_clock_skew_us` distribution.

## UnknownMessagesDropped
**Severity: info.**

Frames that match no known message type are being dropped
(`pb_unknown_messages_dropped_total`). A sustained nonzero rate usually means the
venue added a new message type; capture a sample frame and add a wire variant.

## WalConsumerLagHigh
**Severity: warning.**

A WAL consumer (e.g. a `serve` replica) is lagging beyond the retention window
(`pb_wal_consumer_lag_bytes`).

1. Identify the lagging consumer (its `consumer_*.pos` file).
2. If it cannot catch up, it will hit a segment gap and must re-hydrate from a
   checkpoint (the serve tailer does this automatically on resync).
3. A permanently-stuck consumer blocks WAL pruning and can lead to disk-full —
   resync or remove it.
