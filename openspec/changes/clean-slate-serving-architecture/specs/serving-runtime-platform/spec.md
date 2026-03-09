## MODIFIED Requirements

### Requirement: Browser serving is separated from venue connectivity

The ingest runtime SHALL own venue connectivity, normalization, sequencing, and
durable writes to both the event log and storage sinks. The serve runtime SHALL
own browser-facing HTTP, WebSocket, and internal read contracts. Communication
between runtimes SHALL occur through the shared event log, not through in-process
channels or direct venue connections.

#### Scenario: Ingest runtime operates without browser serving

- **WHEN** the ingest runtime is running
- **THEN** it SHALL connect to venue WebSocket feeds, normalize messages, and write to the event log and storage
- **AND** it SHALL NOT run any HTTP or WebSocket server for browser clients

#### Scenario: Serve runtime operates without venue connectivity

- **WHEN** the serve runtime is running
- **THEN** it SHALL tail the event log, maintain read models, and serve browser clients
- **AND** it SHALL NOT establish any direct venue WebSocket connections

#### Scenario: Backward-compatible monolith mode

- **WHEN** the operator runs `poly-book all` or the legacy `serve-api` command
- **THEN** both ingest and serve functions SHALL run in a single process
- **AND** the system SHALL behave identically to the current `serve-api` runtime

### Requirement: Serving replicas remain stateless

Serving replicas SHALL be stateless beyond in-memory caches and event log
read positions. Any replica SHALL be able to start, hydrate from a checkpoint,
and begin serving without depending on unique mutable state from a prior
instance.

#### Scenario: Replica replacement without state transfer

- **WHEN** a serve replica is stopped and a new replica starts
- **THEN** the new replica SHALL hydrate from the latest checkpoint and event log
- **AND** it SHALL begin serving without requiring state transfer from the stopped replica

#### Scenario: Multiple replicas serve concurrently

- **WHEN** multiple serve replicas are deployed
- **THEN** each replica SHALL independently tail the event log and maintain its own read model
- **AND** all replicas SHALL serve consistent data within the event log propagation window
