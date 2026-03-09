# Crate Documentation — Spec

## ADDED Requirements

### Requirement: Every crate SHALL have a README.md

Each crate under `crates/` SHALL have a `README.md` at its root directory
containing: purpose, key types, data flow, design notes, and a docs-update table.

#### Scenario: Agent reads crate README before modification

- **WHEN** an AI agent is tasked with modifying a crate
- **THEN** the crate's `README.md` SHALL exist and provide sufficient context
  to understand the crate's purpose, public API surface, and upstream/downstream
  dependencies

#### Scenario: Human developer browses crate directory

- **WHEN** a developer opens a crate directory in a file browser or IDE
- **THEN** a `README.md` SHALL be present with a one-paragraph purpose statement
  and a key-types listing

### Requirement: Each crate README SHALL include a docs-update table

Each crate `README.md` SHALL contain a "Docs to Update After Changes" section
with a table mapping change categories to the documents and artifacts that
MUST be updated.

#### Scenario: Agent changes a crate and propagates docs

- **WHEN** an AI agent modifies a crate's public types, routes, schema, or config
- **THEN** the agent SHALL consult the crate's docs-update table and propagate
  changes to all listed targets (docs/, CLAUDE.md, config, other crates, OpenSpec)

#### Scenario: Update table includes OpenSpec propagation

- **WHEN** a crate README's update table covers an externally-visible boundary
  (routes, schemas, CLI commands, scope changes)
- **THEN** the table SHALL include rows referencing the active OpenSpec change
  artifacts (tasks.md, proposal.md, design.md, or capability spec.md)

### Requirement: Each lib.rs SHALL have a module-level doc comment

Each `lib.rs` (or `main.rs` for pb-bin) SHALL contain a `//!` doc comment
with a one-sentence purpose statement and a reference to the crate README.

#### Scenario: cargo doc shows meaningful module description

- **WHEN** a developer runs `cargo doc --workspace --no-deps --open`
- **THEN** each crate's module page SHALL display a one-sentence description
  and a link to the README for detailed documentation

### Requirement: Centralized architecture diagram SHALL exist

A `docs/architecture.md` file SHALL contain the system-wide data flow diagram,
crate dependency graph, and runtime topology.

#### Scenario: CLAUDE.md and README.md reference centralized diagram

- **WHEN** an agent or developer needs the system architecture overview
- **THEN** CLAUDE.md and README.md SHALL reference `docs/architecture.md` as the
  single source of truth rather than maintaining independent inline copies

#### Scenario: Architecture diagram reflects actual crate dependencies

- **WHEN** `docs/architecture.md` is read
- **THEN** the crate dependency graph SHALL match the actual dependency
  relationships defined in the workspace `Cargo.toml` files

### Requirement: CLAUDE.md SHALL reference per-crate documentation convention

CLAUDE.md SHALL contain a section instructing agents to read the crate README
before modifying any crate and to follow the docs-update table after changes.

#### Scenario: New agent session follows convention

- **WHEN** a new AI agent session begins and CLAUDE.md is loaded
- **THEN** the agent SHALL be instructed to read the relevant crate README
  before modifying that crate and to check the update table afterward
