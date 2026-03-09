## ADDED Requirements

### Requirement: Checkpoints include WAL offset

The `BookCheckpoint` record SHALL include the WAL offset at which the checkpoint
was taken. This offset SHALL be used to coordinate hydration on serve startup.

#### Scenario: Checkpoint records WAL position

- **WHEN** the checkpoint writer produces a periodic book checkpoint
- **THEN** the checkpoint SHALL include the current WAL write offset as a `u64` field
- **AND** the checkpoint SHALL include the full book state for all active assets at that offset

#### Scenario: Checkpoint WAL offset is monotonically increasing

- **WHEN** successive checkpoints are written
- **THEN** each checkpoint's WAL offset SHALL be greater than or equal to the previous checkpoint's offset

### Requirement: Serve runtime hydrates from checkpoint on startup

The serve runtime SHALL load the latest checkpoint and replay the event log from
the checkpoint's WAL offset to the current head before reporting readiness.

#### Scenario: Cold start hydration from checkpoint

- **WHEN** the serve runtime starts with no prior in-memory state
- **THEN** it SHALL read the latest checkpoint from storage
- **AND** restore per-asset book state from the checkpoint
- **AND** seek the event log to the checkpoint's WAL offset
- **AND** replay all events from that offset to the current log head

#### Scenario: Serve runtime reports readiness only after hydration

- **WHEN** the serve runtime is hydrating from a checkpoint
- **THEN** it SHALL NOT accept client connections or report readiness on health endpoints
- **AND** it SHALL report readiness only after hydration is complete and live tailing has begun

#### Scenario: Hydration completes in bounded time

- **WHEN** the serve runtime hydrates from the latest checkpoint
- **THEN** the hydration window SHALL be bounded by the checkpoint interval
- **AND** typical hydration SHALL complete in under 100 milliseconds for normal checkpoint intervals

### Requirement: Graceful degradation without checkpoint

The serve runtime SHALL handle the case where no checkpoint is available (first
deployment or checkpoint data loss).

#### Scenario: Startup without checkpoint falls back to live feed

- **WHEN** the serve runtime starts and no checkpoint is available in storage
- **THEN** it SHALL begin tailing the event log from the earliest available offset
- **AND** it SHALL build book state from events as they arrive
- **AND** it SHALL log a warning that hydration is running without a checkpoint

#### Scenario: Startup without WAL falls back to feed-only mode

- **WHEN** the serve runtime starts and no event log is available
- **THEN** it SHALL fall back to the current behavior of building state from live feed events
- **AND** it SHALL log a warning that it is running without WAL hydration
