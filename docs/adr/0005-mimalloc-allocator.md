# ADR-0005: mimalloc as Global Allocator

## Status
Accepted

## Context
The default glibc allocator has known performance characteristics that are
suboptimal for allocation-heavy async Rust workloads:

- Thread-local caching is less effective under tokio's work-stealing scheduler
- Fragmentation under sustained small allocations (channel buffers, serde)
- Higher latency variance (p99) compared to modern allocators

Alternatives considered: jemalloc, tcmalloc, mimalloc.

## Decision
Use `mimalloc` as the global allocator in the binary crate (`pb-bin`).

```rust
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
```

## Consequences
- **Latency**: mimalloc provides more consistent allocation latency with lower
  p99 compared to glibc malloc. Particularly beneficial for the Dispatcher hot
  path.
- **Throughput**: 5–15% improvement in allocation-heavy benchmarks (channel
  send/recv, serde deserialization, BTreeMap operations).
- **Memory overhead**: Slightly higher RSS due to mimalloc's segment-based
  design. Acceptable for a server process.
- **Portability**: mimalloc builds on Linux, macOS, and Windows. No platform
  restrictions for this project.
- **Library crates unaffected**: Only the binary crate sets the global
  allocator. Library crates remain allocator-agnostic.
