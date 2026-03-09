## MODIFIED Requirements

### Requirement: Per-asset broadcast partitioning for WS streaming

The WebSocket streaming system SHALL maintain one broadcast channel per active
asset. WS subscribers SHALL receive updates only for their subscribed asset
without client-side filtering.

#### Scenario: Subscriber receives only subscribed asset updates

- **WHEN** a WS client subscribes to asset "btc-5m"
- **THEN** the client SHALL receive book updates only for "btc-5m"
- **AND** updates for other assets SHALL NOT be sent to this client's connection

#### Scenario: Per-asset broadcast channel created on activation

- **WHEN** a new asset becomes active during market rotation
- **THEN** the system SHALL create a dedicated broadcast channel for that asset

#### Scenario: Per-asset broadcast channel dropped on deactivation

- **WHEN** an asset is removed from the active set during market rotation
- **THEN** the system SHALL drop the broadcast channel for that asset
- **AND** any connected WS subscribers for that asset SHALL receive a close frame

#### Scenario: Slow subscriber resync within per-asset channel

- **WHEN** a WS subscriber on asset "btc-5m" falls behind the broadcast buffer
- **THEN** the system SHALL detect the lag on that asset's channel
- **AND** send a full resync snapshot for "btc-5m" to the lagged subscriber
