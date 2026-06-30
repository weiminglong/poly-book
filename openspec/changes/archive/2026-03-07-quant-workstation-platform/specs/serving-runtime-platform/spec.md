# Spec: Serving Runtime Platform

## Runtime Separation

### Scenario: Browser serving is separated from venue connectivity

```
Given the workstation platform is deployed beyond a single-process development runtime
When the serving architecture is evaluated
Then venue connectivity, normalization, sequencing, and durable writes belong to an ingest runtime
And browser-facing HTTP, WebSocket, and internal read contracts belong to a separate serve runtime
```

### Scenario: Serving replicas remain stateless

```
Given multiple serve runtime replicas are deployed behind a load balancer
When a replica starts, stops, or is replaced
Then the replica rebuilds live read state from durable checkpoints and ordered updates
And workstation correctness does not depend on any single serve process retaining unique mutable state
```

## Live State Hydration

### Scenario: Live read state hydrates from durable state before becoming ready

```
Given the serve runtime depends on stored checkpoints and ordered live updates
When a serve replica starts or recovers
Then it hydrates its live read model from durable checkpoints and subsequent ordered updates
And the replica does not report readiness until that hydration is complete
```

### Scenario: Slow consumers receive bounded resync behavior

```
Given a client falls behind the live update rate
When the serve runtime detects the client has exceeded its stream buffer
Then the runtime may drop incremental updates for that client
And it sends or requires an explicit resync snapshot rather than letting continuity silently drift
```

## Transport Boundaries

### Scenario: Exposed read surfaces require bearer authentication

```
Given the workstation read surfaces are bound beyond loopback
When HTTP, WebSocket, or gRPC data routes are served
Then startup requires a configured bearer token
And clients must present that token while health probes remain available for orchestrators
```

### Scenario: Browser and internal consumers use different transports over one service layer

```
Given the workstation must serve both browser clients and internal services
When read surfaces are exposed
Then browser clients use versioned HTTP and WebSocket contracts
And internal services may use gRPC over the same transport-neutral serving domain
```

### Scenario: Browser support is not forced through gRPC-specific constraints

```
Given the workstation includes a browser-facing SPA
When internal gRPC support is added
Then browser transport remains HTTP and WebSocket compatible
And browser access does not require gRPC-Web or proxy-specific behavior to remain functional
```

## Historical Serving

### Scenario: Interactive historical reads use a serving-oriented backend

```
Given replay, integrity, execution, and query workloads may be interactive and concurrent
When the deployed workstation serves historical requests
Then interactive reads use an approved serving backend such as ClickHouse
And the browser-serving runtime does not rely on scanning archival files for normal interactive workloads
```

### Scenario: Parquet remains replay and audit truth

```
Given the workstation uses a serving-oriented backend for interactive reads
When replay correctness, validation, or recovery questions arise
Then Parquet remains available as the canonical audit and replay-truth source
And the serving backend does not redefine historical truth by itself
```
