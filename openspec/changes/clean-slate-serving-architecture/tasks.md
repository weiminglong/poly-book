## 1. Per-Asset Broadcast Partitioning (Phase 6.0)

- [x] 1.1 Replace the single `BookBroadcast` with a `PerAssetBroadcast` struct backed by `FxHashMap<AssetId, broadcast::Sender<BookUpdateMessage>>` in `pb-api/src/streaming.rs`
- [x] 1.2 Update `BookProjector` / `LiveReadModel` to send updates to the per-asset broadcast channel instead of the global broadcast
- [x] 1.3 Update `ws_orderbook` handler to subscribe to the asset-specific broadcast channel, removing the per-message asset_id filter loop
- [x] 1.4 Add lifecycle management: create broadcast channels when assets activate, drop them on rotation, send WS close frames to subscribers of deactivated assets
- [x] 1.5 Update existing streaming tests to validate per-asset subscription routing and lagged-subscriber resync behavior
- [x] 1.6 Verify `cargo test -p pb-api` passes and manual WS test with multiple assets confirms isolation

## 2. Embedded Event Log — pb-wal Crate (Phase 6.1)

- [x] 2.1 Create `crates/pb-wal/` crate with `Cargo.toml` (deps: `memmap2`, `crc32c`, `bincode`, `thiserror`, `tracing`) and add to workspace
- [x] 2.2 Implement `Segment` struct: mmap'd fixed-size file, append with length-prefix + CRC32C framing, read with checksum verification
- [x] 2.3 Implement `WalWriter`: manages active segment, rotates on size threshold, seals completed segments as read-only
- [x] 2.4 Implement `WalReader`: consumer with independent position tracking, tails across segments, resumes from committed offset
- [x] 2.5 Implement `WalPruner`: removes sealed segments that all registered consumers have advanced past
- [x] 2.6 Define `WalConfig` (segment_size, base_path, max_segments) and `WalError` types
- [x] 2.7 Implement `PersistedRecord` serialization/deserialization via bincode with a version byte prefix
- [x] 2.8 Add unit tests: append-read round-trip, segment rotation, CRC corruption detection, multi-consumer independent positions
- [x] 2.9 Add property tests (`proptest`): arbitrary record sequences survive write-read cycle, segment rotation preserves ordering
- [x] 2.10 Add fuzz target: random byte corruption of WAL segments, verify reader detects and skips corrupt records
- [x] 2.11 Verify `cargo test -p pb-wal` and `cargo check --workspace` pass

## 3. Watch-Based Read Model (Phase 6.2)

- [x] 3.1 Define `AssetSnapshot` struct containing full book state, metadata, and staleness info for a single asset
- [x] 3.2 Refactor `LiveReadModel` internals: replace `Arc<RwLock<LiveState>>` with a single-writer task that owns `HashMap<AssetId, L2Book>` and per-asset `watch::Sender<Arc<AssetSnapshot>>`
- [x] 3.3 Update `spawn_consumer` and `spawn_consumer_with_broadcast` to use the new single-writer projection loop
- [x] 3.4 Update `feed_status_raw()`, `active_assets()`, `snapshot()`, and `is_asset_active()` to read from `watch` receivers instead of acquiring `RwLock`
- [x] 3.5 Update `set_active_assets()` to create/drop watch channels for activated/deactivated assets
- [x] 3.6 Update all existing `LiveReadModel` tests to validate behavior under the new watch-based architecture
- [x] 3.7 Add a test confirming readers see consistent snapshots (no partially applied deltas) under concurrent writer load
- [x] 3.8 Benchmark read latency before/after: measure p50/p99 `snapshot()` call duration under concurrent writes

## 4. Checkpoint Hydration (Phase 6.3)

- [x] 4.1 Extend `BookCheckpoint` in `pb-types` with an optional `wal_offset: Option<u64>` field
- [x] 4.2 Update `pb-store` checkpoint writer to capture and persist the current WAL write offset when producing checkpoints
- [x] 4.3 Update Parquet schema for `BookCheckpoint` to include the `wal_offset` column
- [x] 4.4 Implement `hydrate_from_checkpoint()` in `pb-api`: load latest checkpoint per asset from Parquet, restore book state, return the WAL offset
- [x] 4.5 Implement `replay_wal_tail()`: seek WAL reader to checkpoint offset, apply all events up to current head, switch to live tailing
- [x] 4.6 Add readiness gate: serve runtime health endpoint returns 503 until hydration completes and live tailing begins
- [x] 4.7 Add fallback path: if no checkpoint exists, tail WAL from earliest offset with a warning log; if no WAL exists, fall back to current feed-only behavior
- [x] 4.8 Add integration test: write checkpoint + WAL segment, start serve runtime, verify book state matches expected hydrated state
- [x] 4.9 Measure cold-start time: checkpoint load + WAL tail replay, verify <100ms for typical 5-min checkpoint interval

## 5. Process Separation (Phase 6.4)

- [x] 5.1 Add `Ingest` subcommand to `pb-bin` that runs venue connectivity, dispatcher, WAL writer, and storage sinks without any HTTP/WS server
- [x] 5.2 Add `Serve` subcommand to `pb-bin` that runs WAL reader, book projector, checkpoint hydration, and HTTP/WS server without venue connectivity
- [x] 5.3 Rename current `ServeApi` to an `All` subcommand that runs both ingest and serve in a single process (backward compatible)
- [x] 5.4 Wire `Ingest` subcommand: WsClient → Dispatcher → WalWriter + ParquetSink + ClickHouseSink, checkpoint writer with WAL offset
- [x] 5.5 Wire `Serve` subcommand: checkpoint hydration → WalReader → BookProjector → watch channels + per-asset broadcasts → axum server
- [x] 5.6 Add shared WAL directory coordination: writer uses atomic segment creation, readers detect new segments via directory polling or inotify
- [x] 5.7 Add graceful shutdown for both processes: ingest flushes WAL and seals active segment, serve commits consumer position
- [x] 5.8 Update `config/default.toml` with `[wal]` section: `base_path`, `segment_size_mb`, `max_segments`
- [ ] 5.9 Test: run `poly-book ingest` and `poly-book serve` as separate processes, verify serve receives events and serves correct book state
- [ ] 5.10 Test: kill and restart `poly-book serve`, verify it hydrates from checkpoint + WAL and resumes serving without data loss
- [ ] 5.11 Test: `poly-book all` behaves identically to current `serve-api` (backward compatibility)

## 6. Service Layer Extraction (Phase 6.5)

- [x] 6.1 Create `crates/pb-service/` crate with `Cargo.toml` (deps: `pb-types`, `pb-book`, `pb-replay`, `thiserror`, `async-trait`) and add to workspace
- [x] 6.2 Define `BookService` trait: `snapshot()`, `feed_status()`, `active_assets()`, `is_asset_active()`
- [x] 6.3 Define `ReplayService` trait: `reconstruct()`
- [x] 6.4 Define `IntegrityService` trait: `summary()`
- [x] 6.5 Define `ExecutionService` trait: `timeline()`
- [x] 6.6 Define `ServiceError` enum with domain-specific variants (NotFound, InvalidParams, Unavailable, Internal)
- [x] 6.7 Implement concrete service structs backed by the watch-based read model, `ParquetReader`, and `ReplayEngine`
- [x] 6.8 Refactor `pb-api` handlers to become thin adapters: parse HTTP input → call service → format HTTP output
- [x] 6.9 Move `ApiError` mapping logic to a `ServiceError → ApiError` conversion layer in `pb-api`
- [x] 6.10 Add unit tests for service implementations without HTTP (direct trait method calls)
- [x] 6.11 Verify all existing `pb-api` tests still pass after refactor

## 7. ClickHouse Interactive Reads (Phase 6.6)

- [x] 7.1 Add `ClickHouseReplayService` implementing `ReplayService` trait, routing interactive replay queries to ClickHouse
- [x] 7.2 Add `ClickHouseIntegrityService` implementing `IntegrityService` trait for interactive integrity queries
- [x] 7.3 Add `ClickHouseExecutionService` implementing `ExecutionService` trait for interactive execution timeline queries
- [x] 7.4 Add configurable service backend selection: `api.historical_backend = "clickhouse" | "parquet"` in config
- [x] 7.5 Wire service selection in `Serve` / `All` subcommand startup based on config
- [x] 7.6 Add fallback: if ClickHouse is unavailable, degrade to Parquet with a warning log
- [ ] 7.7 Add integration tests: verify replay, integrity, and execution queries return equivalent results from both backends
- [ ] 7.8 Benchmark: compare query latency between ClickHouse and Parquet backends for typical workstation queries

## 8. Documentation and CI

- [x] 8.1 Add `crates/pb-wal/README.md` with purpose, design, usage, and segment layout diagram
- [x] 8.2 Add `crates/pb-service/README.md` with purpose, trait inventory, and transport adapter pattern
- [x] 8.3 Update `docs/architecture.md` with the new ingest/serve/WAL topology diagram
- [x] 8.4 Update `docs/serve-api.md` to document checkpoint hydration, process separation, and the `all` backward-compatible mode
- [x] 8.5 Update `docs/operations.md` with commands for running ingest and serve separately, WAL configuration, and health endpoint semantics
- [x] 8.6 Update `CLAUDE.md` crate table with `pb-wal` and `pb-service` entries
- [x] 8.7 Add WAL fuzz target to CI workflow
- [x] 8.8 Add benchmark regression gate for read model latency (p99 snapshot read)
