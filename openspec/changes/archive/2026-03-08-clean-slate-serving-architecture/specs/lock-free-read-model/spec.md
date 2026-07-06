## ADDED Requirements

### Requirement: Single-writer book projection

The system SHALL maintain book state through a single-writer task that owns all
`L2Book` mutations. No other task or thread SHALL directly mutate book state.

#### Scenario: Writer applies deltas exclusively

- **WHEN** a book delta event arrives from the event log
- **THEN** exactly one writer task SHALL apply the delta to the corresponding `L2Book`
- **AND** no concurrent task SHALL hold a mutable reference to that book

#### Scenario: Writer applies snapshots exclusively

- **WHEN** a book snapshot completes materialization
- **THEN** the writer task SHALL replace the book state atomically
- **AND** readers SHALL see either the complete old state or the complete new state, never a partial mix

### Requirement: Lock-free read access via watch channels

The system SHALL publish the latest book state for each active asset through a
`tokio::sync::watch` channel. HTTP and WS handlers SHALL read the latest state
without acquiring any lock.

#### Scenario: HTTP handler reads latest state without blocking

- **WHEN** an HTTP handler serves a book snapshot request
- **THEN** it SHALL obtain the latest published state via `watch::Receiver::borrow()`
- **AND** the read SHALL NOT block on or contend with the writer task

#### Scenario: Readers see consistent snapshots

- **WHEN** a reader borrows the current state from a watch channel
- **THEN** the state SHALL represent a complete, consistent book snapshot
- **AND** the snapshot SHALL NOT contain partially applied deltas

#### Scenario: Natural batching under load

- **WHEN** the writer commits multiple deltas between consecutive reader polls
- **THEN** readers SHALL see only the latest committed state
- **AND** intermediate states SHALL be skipped without error

### Requirement: Per-asset watch channels

The system SHALL maintain one `watch` channel per active asset. Watch channels
SHALL be created when an asset becomes active and dropped when an asset rotates
out.

#### Scenario: Watch channel created for new active asset

- **WHEN** market rotation adds a new asset to the active set
- **THEN** the system SHALL create a new watch channel for that asset

#### Scenario: Watch channel dropped for rotated-out asset

- **WHEN** market rotation removes an asset from the active set
- **THEN** the system SHALL drop the watch channel for that asset
- **AND** any receivers still borrowing the last published state SHALL continue to hold a valid reference until dropped
