//! Centralized timestamp-unit normalization.
//!
//! Venue and snapshot payloads carry timestamps in inconsistent resolutions
//! (seconds, milliseconds, microseconds, nanoseconds). Previously two separate
//! ad-hoc converters (the dispatcher and the REST backfill) handled only the
//! millisecond case, so a seconds-resolution value was under-scaled into a 1970
//! partition and a nanosecond value was kept as an absurd far-future microsecond
//!. This module is the single converter used by both
//! paths, classifying by magnitude against the contemporary epoch.

/// Normalize a raw integer timestamp to **microseconds**, inferring its source
/// resolution from magnitude.
///
/// Reference: the contemporary epoch is ~1.7e9 s ≈ 1.7e12 ms ≈ 1.7e15 µs ≈
/// 1.7e18 ns, so the magnitude bands below cleanly separate the four resolutions
/// for any plausible present-day timestamp:
///
/// | range                | assumed unit | scaling   |
/// |----------------------|--------------|-----------|
/// | `0`                  | unknown      | `0`       |
/// | `1 .. <1e12`         | seconds      | `×1_000_000` |
/// | `1e12 .. <1e15`      | milliseconds | `×1_000`  |
/// | `1e15 .. <1e18`      | microseconds | unchanged |
/// | `>=1e18`             | nanoseconds  | `÷1_000`  |
///
/// `0` is preserved as a sentinel for "no/unknown timestamp" rather than being
/// scaled, so callers can detect and route it (e.g. to a quarantine partition).
pub fn normalize_to_micros(raw: u64) -> u64 {
    const MS_THRESHOLD: u64 = 1_000_000_000_000; // 1e12
    const US_THRESHOLD: u64 = 1_000_000_000_000_000; // 1e15
    const NS_THRESHOLD: u64 = 1_000_000_000_000_000_000; // 1e18

    if raw == 0 {
        0
    } else if raw < MS_THRESHOLD {
        raw.saturating_mul(1_000_000) // seconds → µs
    } else if raw < US_THRESHOLD {
        raw.saturating_mul(1_000) // milliseconds → µs
    } else if raw < NS_THRESHOLD {
        raw // already microseconds
    } else {
        raw / 1_000 // nanoseconds → µs
    }
}

/// Parse a string timestamp into microseconds, returning `None` for absent or
/// non-numeric input. Numeric values are normalized via [`normalize_to_micros`].
pub fn parse_to_micros(ts: Option<&str>) -> Option<u64> {
    Some(normalize_to_micros(ts?.parse::<u64>().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Contemporary epoch in each resolution (2025-ish).
    const SECS: u64 = 1_750_000_000;
    const MILLIS: u64 = 1_750_000_000_000;
    const MICROS: u64 = 1_750_000_000_000_000;
    const NANOS: u64 = 1_750_000_000_000_000_000;

    #[test]
    fn all_resolutions_normalize_to_same_micros() {
        assert_eq!(normalize_to_micros(SECS), MICROS);
        assert_eq!(normalize_to_micros(MILLIS), MICROS);
        assert_eq!(normalize_to_micros(MICROS), MICROS);
        assert_eq!(normalize_to_micros(NANOS), MICROS);
    }

    #[test]
    fn zero_is_preserved_as_sentinel() {
        assert_eq!(normalize_to_micros(0), 0);
    }

    #[test]
    fn parse_handles_none_and_garbage() {
        assert_eq!(parse_to_micros(None), None);
        assert_eq!(parse_to_micros(Some("not-a-number")), None);
        assert_eq!(parse_to_micros(Some("0")), Some(0));
    }

    #[test]
    fn parse_normalizes_each_unit() {
        assert_eq!(parse_to_micros(Some(&SECS.to_string())), Some(MICROS));
        assert_eq!(parse_to_micros(Some(&MILLIS.to_string())), Some(MICROS));
        assert_eq!(parse_to_micros(Some(&MICROS.to_string())), Some(MICROS));
        assert_eq!(parse_to_micros(Some(&NANOS.to_string())), Some(MICROS));
    }

    #[test]
    fn saturates_instead_of_overflowing() {
        // A pathological large "seconds" value must not panic on ×1e6.
        assert_eq!(normalize_to_micros(u64::MAX), u64::MAX / 1_000);
    }
}
