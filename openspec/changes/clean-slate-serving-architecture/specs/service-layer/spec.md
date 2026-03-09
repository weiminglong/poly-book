## ADDED Requirements

### Requirement: Transport-neutral service traits

The system SHALL define domain service traits that encapsulate business logic
independently of any transport framework. Service implementations SHALL not
depend on axum, tonic, or any HTTP/gRPC-specific types.

#### Scenario: Service trait is usable from HTTP handler

- **WHEN** an axum HTTP handler receives a request
- **THEN** it SHALL parse transport-specific input, call the service trait method, and format the response
- **AND** no domain logic SHALL exist in the handler itself

#### Scenario: Service trait is usable from gRPC handler

- **WHEN** a future tonic gRPC handler receives a request
- **THEN** it SHALL parse transport-specific input, call the same service trait method, and format the response
- **AND** the service implementation SHALL be shared with the HTTP path

#### Scenario: Service is testable without transport

- **WHEN** service logic is tested
- **THEN** tests SHALL call service methods directly without constructing HTTP requests or responses
- **AND** service errors SHALL be transport-neutral domain errors

### Requirement: Domain error types separate from transport errors

The service layer SHALL define its own error types that represent domain failures.
Transport layers SHALL map domain errors to transport-specific error codes.

#### Scenario: Domain error maps to HTTP status

- **WHEN** a service method returns a `ServiceError::NotFound` variant
- **THEN** the HTTP handler SHALL map it to HTTP 404
- **AND** the mapping SHALL be defined in the transport adapter, not the service

#### Scenario: Domain error maps to gRPC status

- **WHEN** a service method returns a `ServiceError::NotFound` variant
- **THEN** a future gRPC handler SHALL map it to gRPC `NOT_FOUND` status
- **AND** the same domain error type SHALL be reusable across transports

### Requirement: Service layer covers all workstation domains

The service layer SHALL provide traits for all current workstation domains:
book snapshots, feed status, active assets, replay reconstruction, integrity
summary, and execution timeline.

#### Scenario: Book service provides live snapshots

- **WHEN** a client requests a live book snapshot
- **THEN** the book service SHALL return the current book state for the requested asset
- **AND** the response SHALL use domain types, not transport DTOs

#### Scenario: Replay service provides historical reconstruction

- **WHEN** a client requests replay reconstruction
- **THEN** the replay service SHALL reconstruct the book at the requested timestamp
- **AND** the service SHALL accept replay parameters as domain types

#### Scenario: Integrity service provides data quality summary

- **WHEN** a client requests an integrity summary
- **THEN** the integrity service SHALL aggregate continuity events, validation outcomes, and completeness labels
- **AND** the response SHALL use domain types
