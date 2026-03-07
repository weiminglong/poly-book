# Spec: Live Market Observability

## Feed Status

### Scenario: Active assets and feed session state are queryable

```
Given the ingest pipeline is running for one or more active assets
When the workstation requests live feed status
Then it receives the active asset set and feed session status
And the response includes the latest observed event time for each active asset or session
```

### Scenario: Freshness is visible

```
Given live market-data events are being observed
When the workstation requests feed status or live book state
Then the response includes freshness metadata derived from the latest observed timestamps
And a consumer can detect when an asset or feed has gone stale
```

## Live Book Views

### Scenario: Live order book snapshot is queryable

```
Given the server maintains a live read model for an active asset
When the workstation requests the current order book snapshot for that asset
Then it receives the current top of book and depth state
And the snapshot is served from the server-side read model rather than browser-side reconstruction
```

### Scenario: Live order book updates are streamable

```
Given a client subscribes to live order book updates for an asset
When the order book changes
Then the workstation receives ordered update messages over a streaming interface
And the messages include enough metadata to identify update time and continuity state
```

## Continuity Visibility

### Scenario: Reconnect and gap warnings are visible with live state

```
Given the feed experiences reconnects, source resets, or sequence gaps
When the workstation requests live feed or order book state
Then the response or stream includes continuity warnings relevant to the affected asset or session
And the operator does not need log access to detect that the live view may be best-effort
```
