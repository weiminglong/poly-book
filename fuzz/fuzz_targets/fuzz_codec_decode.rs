#![no_main]
//! Fuzz the WAL frame codec's decode path against arbitrary bytes.
//!
//! `codec::decode` is the durability deserialization path: it parses a version
//! byte then bincode-decodes a `PersistedRecord`. The `WalReader` only ever hands
//! it CRC-validated frames, but a version mismatch, a truncated payload, or a
//! corrupt-but-CRC-colliding frame can still reach it — and on any input it must
//! return `Err`, never panic, OOM, or hang. This drives it with unconstrained
//! bytes to prove that.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Must not panic on any input — malformed frames are an error, not a crash.
    let _ = pb_wal::codec::decode(data);
});
