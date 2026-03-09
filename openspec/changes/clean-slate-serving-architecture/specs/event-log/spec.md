## ADDED Requirements

### Requirement: Durable append-only event log

The system SHALL provide an embedded write-ahead log that durably persists all
`PersistedRecord` events in write order. The log SHALL survive process crashes
and allow consumers to resume from their last committed position.

#### Scenario: Events survive process crash

- **WHEN** the ingest runtime appends events to the event log and the process crashes
- **THEN** all events whose append call returned success SHALL be recoverable on restart
- **AND** partially written records (incomplete CRC) SHALL be detected and skipped

#### Scenario: Append returns only after durable write

- **WHEN** a `PersistedRecord` is appended to the event log
- **THEN** the append call SHALL not return success until the record is written to the mmap'd segment file
- **AND** the record SHALL be framed with a length prefix and CRC32C checksum

### Requirement: Segment-based storage with rotation

The event log SHALL store records in fixed-size segment files. When the active
segment exceeds the configured segment size, the log SHALL seal it as read-only
and open a new active segment.

#### Scenario: Segment rotation on size threshold

- **WHEN** the active segment's written bytes exceed the configured segment size
- **THEN** the log SHALL seal the current segment as read-only
- **AND** open a new segment file for subsequent appends
- **AND** the transition SHALL be atomic with no lost or duplicated records

#### Scenario: Sealed segments are read-only

- **WHEN** a segment is sealed after rotation
- **THEN** no further writes SHALL occur to that segment
- **AND** consumers SHALL be able to mmap and read it concurrently with new writes to the active segment

### Requirement: Multi-consumer tailing with independent positions

The event log SHALL support multiple concurrent consumers, each maintaining an
independent read position. Consumers SHALL be able to tail the log from any
valid offset.

#### Scenario: Multiple consumers read at different rates

- **WHEN** consumer A is at offset 1000 and consumer B is at offset 5000
- **THEN** both consumers SHALL receive all records from their respective positions forward
- **AND** neither consumer's read rate SHALL affect the other

#### Scenario: Consumer resumes from last committed position

- **WHEN** a consumer restarts after a crash
- **THEN** it SHALL resume reading from its last committed position
- **AND** it SHALL not miss any records that were appended after that position

### Requirement: Segment pruning after all consumers advance

The event log SHALL prune sealed segments that all registered consumers have
fully read past. Pruning SHALL NOT remove segments that any consumer still
needs.

#### Scenario: Segment pruned when all consumers advance past it

- **WHEN** all registered consumers have committed positions beyond the end of a sealed segment
- **THEN** the log MAY delete that segment file to reclaim disk space

#### Scenario: Segment retained while any consumer needs it

- **WHEN** at least one consumer's committed position is within a sealed segment
- **THEN** the log SHALL NOT delete that segment

### Requirement: Integrity verification on read

The event log SHALL verify the CRC32C checksum of each record on read. Corrupt
records SHALL be detected and reported.

#### Scenario: Corrupt record detected on read

- **WHEN** a consumer reads a record whose payload does not match its CRC32C checksum
- **THEN** the log SHALL return an integrity error for that record
- **AND** the consumer SHALL be able to skip the corrupt record and continue reading
