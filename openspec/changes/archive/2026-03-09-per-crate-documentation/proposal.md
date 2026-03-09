# Per-Crate Documentation

## Why

The workspace has 8 crates with zero crate-level documentation — no README files,
no module-level doc comments. All context lives in root-level CLAUDE.md and `docs/`.
AI agents modifying a single crate have no local orientation and no guidance on
which docs to update after changes. Human developers browsing a crate directory
have no entry point beyond reading source code.

## What Changes

- Add a `README.md` to each of the 8 crates with purpose, key types, data flow
  diagrams, design notes, and a "Docs to Update After Changes" table.
- Add a centralized `docs/architecture.md` with the system-wide data flow diagram,
  crate dependency graph, and runtime topology.
- Add `//!` doc comments in each `lib.rs`/`main.rs` for `cargo doc` discoverability.
- Update CLAUDE.md with the per-crate documentation convention so future agents
  know to read crate READMEs and follow update tables.
- Update README.md to reference `docs/architecture.md` from the architecture section.

## Capabilities

### New Capabilities

- `crate-documentation`: Per-crate README files with purpose, key types, data flow,
  design notes, and doc-update guidance for AI agents and human developers.

### Modified Capabilities

(none — this is a documentation-only change with no behavior modifications)

## Impact

- 10 new markdown files (1 in `docs/`, 8 in `crates/*/`, 1 OpenSpec artifact set)
- 10 modified files (CLAUDE.md, README.md, 8 `lib.rs` files with `//!` comments)
- No code changes, no dependency changes, no test changes
- Future AI agents and human contributors get faster crate-level onboarding
- Future changes get explicit doc propagation checklists per crate
