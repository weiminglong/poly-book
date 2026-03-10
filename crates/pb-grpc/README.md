# pb-grpc

gRPC read surface for the poly-book workstation. Provides programmatic access
to historical query services for internal consumers.

## Purpose

Exposes the same `pb-service` traits used by the HTTP API through a tonic-based
gRPC transport. This allows internal tools and services to query replay,
integrity, and execution data without going through HTTP.

## Key Types

- `GrpcWorkstationService` — implements the `WorkstationService` tonic trait,
  delegating to `AnyReplayService`, `AnyIntegrityService`, `AnyExecutionService`
- `start_grpc_server()` — launches the gRPC server with graceful shutdown

## RPCs

| RPC | Description |
|-----|-------------|
| `Reconstruct` | Historical book reconstruction at a target timestamp |
| `IntegritySummary` | Data integrity assessment for an asset time window |
| `ExecutionTimeline` | Execution event timeline for an order |

## Configuration

Opt-in via `config/default.toml`:

```toml
[grpc]
enabled = false
listen_addr = "0.0.0.0:50051"
```

## Design Notes

- ServiceError maps to gRPC status codes (NotFound, InvalidArgument, etc.)
- Backend selection (Parquet/ClickHouse) applies equally to gRPC and HTTP
- Proto definitions live in `proto/workstation.proto`

## Docs to Update After Changes

| What changed | Update |
|--------------|--------|
| Proto schema | `proto/workstation.proto`, regenerate with `cargo build -p pb-grpc` |
| New RPC | `src/lib.rs`, `docs/serve-api.md`, `CLAUDE.md` |
| Config keys | `config/default.toml`, `docs/operations.md` |
