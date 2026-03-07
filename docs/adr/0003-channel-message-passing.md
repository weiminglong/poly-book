# ADR-0003: Channel-Based Message Passing (No Locks on Hot Path)

## Status
Accepted

## Context
The system has a clear data flow: WebSocket → Dispatcher → Storage/API. The
hot path processes every market data message. Shared-state concurrency
(Arc<Mutex<_>>) introduces lock contention, priority inversion, and complex
deadlock reasoning.

## Decision
Components communicate exclusively via bounded `tokio::mpsc` channels. The
Dispatcher runs single-threaded — no locks on the book update path.

```
WS → mpsc → Dispatcher → mpsc → ParquetSink
                       → mpsc → ClickHouseSink
                       → mpsc → LiveReadModel
```

## Consequences
- **Latency**: No lock acquisition on the hot path. Channel send is a CAS on
  the internal ring buffer — typically <100 ns.
- **Backpressure**: Bounded channels provide natural backpressure. If a sink
  falls behind, the channel fills and the Dispatcher blocks, preventing
  unbounded memory growth.
- **Failure isolation**: A panicking sink doesn't poison a mutex; the
  channel simply closes and the Dispatcher observes `SendError`.
- **Trade-off**: Cross-component queries require message round-trips or
  separate read models rather than direct shared-state reads. Accepted because
  the system is write-heavy and read queries go through dedicated API endpoints.
- **Ordering**: Single-producer channels preserve message order within an
  asset. Multi-asset ordering is maintained by timestamp-based sorting in the
  replay engine.
