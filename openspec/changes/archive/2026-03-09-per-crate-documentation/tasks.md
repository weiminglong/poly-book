# Per-Crate Documentation — Tasks

## 1. Centralized Architecture Diagram

- [x] 1.1 Create `docs/architecture.md` with system data flow diagram, crate dependency graph, persisted record model overview, and runtime topology
- [x] 1.2 Update `CLAUDE.md` Architecture section to reference `docs/architecture.md` instead of inline-only diagram
- [x] 1.3 Update `README.md` Architecture section to reference `docs/architecture.md`

## 2. Crate READMEs

- [x] 2.1 Create `crates/pb-types/README.md` — foundation types, persisted record model, fixed-point scaling, docs-update table
- [x] 2.2 Create `crates/pb-book/README.md` — L2Book design, BTreeMap layout, method surface, docs-update table
- [x] 2.3 Create `crates/pb-feed/README.md` — ingest pipeline data flow, WsClient/Dispatcher/RestClient, docs-update table
- [x] 2.4 Create `crates/pb-store/README.md` — storage sinks, schema functions, flush intervals, docs-update table
- [x] 2.5 Create `crates/pb-replay/README.md` — replay engine, EventReader trait, backfill, docs-update table
- [x] 2.6 Create `crates/pb-api/README.md` — route table, LiveReadModel, WebSocket streaming, docs-update table
- [x] 2.7 Create `crates/pb-metrics/README.md` — metrics setup pattern, recorder install, docs-update table
- [x] 2.8 Create `crates/pb-bin/README.md` — CLI subcommands, config layering, mimalloc, docs-update table

## 3. Module-Level Doc Comments

- [x] 3.1 Add `//!` doc comment to `crates/pb-types/src/lib.rs` with purpose and README reference
- [x] 3.2 Add `//!` doc comment to `crates/pb-book/src/lib.rs` with purpose and README reference
- [x] 3.3 Add `//!` doc comment to `crates/pb-feed/src/lib.rs` with purpose and README reference
- [x] 3.4 Add `//!` doc comment to `crates/pb-store/src/lib.rs` with purpose and README reference
- [x] 3.5 Add `//!` doc comment to `crates/pb-replay/src/lib.rs` with purpose and README reference
- [x] 3.6 Add `//!` doc comment to `crates/pb-api/src/lib.rs` with purpose and README reference
- [x] 3.7 Add `//!` doc comment to `crates/pb-metrics/src/lib.rs` with purpose and README reference
- [x] 3.8 Add `//!` doc comment to `crates/pb-bin/src/main.rs` with purpose and README reference

## 4. CLAUDE.md Convention

- [x] 4.1 Add "Per-Crate Documentation" section to CLAUDE.md instructing agents to read crate READMEs and follow docs-update tables

## 5. Verification

- [x] 5.1 Run `cargo check` to confirm `//!` comments compile
- [x] 5.2 Run `cargo test --workspace --exclude pb-integration-tests` to confirm no regressions
- [x] 5.3 Verify `docs/architecture.md` dependency graph matches actual Cargo.toml dependencies
