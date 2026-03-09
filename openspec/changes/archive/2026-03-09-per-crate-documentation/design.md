# Per-Crate Documentation — Design

## Context

The poly-book workspace has 8 crates under `crates/` with no crate-level
documentation. System context is spread across CLAUDE.md (agent instructions),
README.md (contributor onboarding), `docs/` (operational guides), and OpenSpec
(feature changes). None of these answer: "what does this specific crate do and
what should I update after changing it?"

The architecture diagram currently lives inline in both CLAUDE.md and README.md,
with slight variations between them.

## Goals / Non-Goals

**Goals:**

- Every crate has a README.md that orients both AI agents and human developers
- Each README includes a "Docs to Update After Changes" table so agents propagate
  changes correctly — including to OpenSpec artifacts
- A single centralized architecture diagram replaces the duplicated inline versions
- `cargo doc` shows a meaningful one-liner per crate module
- CLAUDE.md instructs agents to read crate READMEs before modifying a crate

**Non-Goals:**

- Comprehensive `///` doc comments on every public type and function (too much
  maintenance burden for the current project stage)
- Crate-level CHANGELOG files (git history and OpenSpec serve this role)
- Publishing individual crates to crates.io (all crates are workspace-internal)
- Restructuring the existing `docs/` directory

## Decisions

### 1. README.md per crate (not just `//!` doc comments)

**Decision**: Each crate gets a `README.md` at its root, with a thin `//!` pointer
in `lib.rs`.

**Rationale**: AI agents discover README.md files predictably via directory listing.
`//!` doc comments are buried inside source files and may be missed during initial
orientation. READMEs also support richer formatting (tables, multi-line diagrams)
than doc comments.

**Alternative considered**: `//!` doc comments only. Rejected because the "Docs to
Update" tables are awkward as Rust doc comments, and agents would need to parse
source files instead of reading a standalone document.

### 2. Consistent README template across all crates

**Decision**: All 8 crate READMEs follow the same section structure: Purpose, Key
Types, Data Flow, Design Notes, Docs to Update After Changes.

**Rationale**: Consistency reduces cognitive load for both humans and agents. An
agent that has read one crate README knows exactly where to look in any other.

### 3. Centralized architecture diagram in `docs/architecture.md`

**Decision**: Create `docs/architecture.md` as the single source of truth for the
system-wide diagram. CLAUDE.md and README.md reference it instead of maintaining
inline copies.

**Rationale**: The current inline diagrams in CLAUDE.md and README.md are slightly
different and will continue to drift. A single file is easier to keep honest.

**Alternative considered**: Keep diagrams inline in both files. Rejected due to
observed drift.

### 4. OpenSpec references in update tables

**Decision**: Crate READMEs that touch externally-visible boundaries (pb-api,
pb-store, pb-replay, pb-bin) include OpenSpec propagation rows in their update
tables. Internal crates (pb-types, pb-book, pb-feed, pb-metrics) include a
lighter note.

**Rationale**: Not every code change warrants an OpenSpec update, but route
additions, schema changes, and scope shifts must be reflected in the active
OpenSpec change. Making this explicit in the update table prevents agents from
forgetting.

### 5. `//!` doc comments limited to one line with README pointer

**Decision**: Each `lib.rs` gets a single `//!` line stating the crate purpose
and linking to `README.md`.

**Rationale**: Avoids duplication between `//!` comments and README content. The
README is the authoritative crate-level document; `cargo doc` simply points there.

## Risks / Trade-offs

**[Staleness]** README files may drift from implementation over time.
→ Mitigated by the "Docs to Update" tables, which remind agents to update
  the README itself when they change the crate. CLAUDE.md reinforces this.

**[Overhead]** 8 new files to maintain.
→ Acceptable because each README is short (under 60 lines) and the update
  tables reduce the larger cost of undocumented change propagation.

**[Diagram drift]** `docs/architecture.md` could drift from actual Cargo.toml
  dependencies.
→ Low risk — crate dependency changes are infrequent and high-visibility.
  The update tables in pb-types and pb-store flag when schema/dependency
  changes need diagram updates.
