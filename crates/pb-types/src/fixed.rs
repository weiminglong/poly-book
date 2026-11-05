use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

use crate::error::TypesError;

const PRICE_SCALE: u32 = 10_000;
const SIZE_SCALE: u64 = 1_000_000;

/// Parse a non-negative decimal string into an integer scaled by `10^decimals`,
/// computed exactly with integer arithmetic — no floating point.
///
/// Returns `None` for: empty input, a sign or any non-digit character,
/// scientific notation, more than `decimals` fractional digits (excess
/// precision is rejected rather than silently rounded), or a value that
/// overflows `u128`. Using `u128` plus checked arithmetic means the result is
/// exact for every value representable by `FixedPrice`/`FixedSize` — the old
/// `f64` path silently lost precision above 2^53 and saturated huge sizes to
/// `u64::MAX`.
fn parse_scaled_decimal(s: &str, decimals: u32) -> Option<u128> {
    if s.is_empty() {
        return None;
    }
    let (int_str, frac_str) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    // "." alone, or any non-digit (including '-', '+', 'e') is invalid.
    if int_str.is_empty() && frac_str.is_empty() {
        return None;
    }
    if !int_str.bytes().all(|b| b.is_ascii_digit()) || !frac_str.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    // Reject more fractional digits than the scale can represent exactly.
    if frac_str.len() > decimals as usize {
        return None;
    }

    let scale = 10u128.pow(decimals);
    let int_val: u128 = if int_str.is_empty() {
        0
    } else {
        int_str.parse().ok()?
    };
    let frac_digits: u128 = if frac_str.is_empty() {
        0
    } else {
        frac_str.parse().ok()?
    };
    // Left-pad the fraction to exactly `decimals` digits, then combine.
    let pad = decimals as usize - frac_str.len();
    let frac_val = frac_digits.checked_mul(10u128.pow(pad as u32))?;
    int_val.checked_mul(scale)?.checked_add(frac_val)
}

/// Fixed-point price representation: value * 10,000.
/// Polymarket prices are 0.00–1.00, so range is 0–10,000.
/// 4 bytes, `Copy`, trivial `Ord`.
///
/// The inner field is private so the `[0, SCALE]` range invariant cannot be
/// violated by constructing `FixedPrice(raw)` or assigning `.0` directly — an
/// out-of-range value would serialize successfully but fail to deserialize,
/// poisoning persisted records. Construct via [`new`],
/// [`from_f64`], or `TryFrom<&str>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FixedPrice(u32);

impl FixedPrice {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(PRICE_SCALE);
    pub const SCALE: u32 = PRICE_SCALE;

    #[inline]
    pub fn new(raw: u32) -> Result<Self, TypesError> {
        if raw > PRICE_SCALE {
            return Err(TypesError::InvalidPrice { raw });
        }
        Ok(Self(raw))
    }

    /// Create from a float (e.g., 0.5 -> FixedPrice(5000))
    pub fn from_f64(v: f64) -> Result<Self, TypesError> {
        if v.is_nan() || v.is_infinite() || v < 0.0 {
            return Err(TypesError::InvalidPriceValue {
                value: v.to_string(),
            });
        }
        let raw = (v * PRICE_SCALE as f64).round() as u32;
        Self::new(raw)
    }

    #[inline]
    pub fn as_f64(self) -> f64 {
        self.0 as f64 / PRICE_SCALE as f64
    }

    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    #[inline]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for FixedPrice {
    /// Exactly four fraction digits, e.g. `0.5000`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:04}", self.0 / PRICE_SCALE, self.0 % PRICE_SCALE)
    }
}

impl TryFrom<&str> for FixedPrice {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let raw = parse_scaled_decimal(s, 4).ok_or_else(|| TypesError::PriceParse {
            input: s.to_string(),
        })?;
        if raw > PRICE_SCALE as u128 {
            return Err(TypesError::InvalidPriceValue {
                value: s.to_string(),
            });
        }
        Ok(Self(raw as u32))
    }
}

/// Human-readable formats (JSON) carry the exact decimal string, so API clients
/// never see binary floating point. Binary formats (bincode in the WAL) carry
/// the raw scaled integer: 4 bytes instead of a length-prefixed string, and no
/// parse on the replay path.
impl Serialize for FixedPrice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.collect_str(self)
        } else {
            serializer.serialize_u32(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for FixedPrice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            deserializer.deserialize_str(FixedPriceVisitor)
        } else {
            let raw = u32::deserialize(deserializer)?;
            FixedPrice::new(raw).map_err(serde::de::Error::custom)
        }
    }
}

struct FixedPriceVisitor;

impl<'de> serde::de::Visitor<'de> for FixedPriceVisitor {
    type Value = FixedPrice;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a decimal string representing a price in [0.0, 1.0]")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        FixedPrice::try_from(v).map_err(serde::de::Error::custom)
    }
}

/// Fixed-point size representation: value * 1,000,000.
/// 8 bytes, `Copy`, trivial `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FixedSize(pub u64);

impl FixedSize {
    pub const ZERO: Self = Self(0);
    pub const SCALE: u64 = SIZE_SCALE;

    #[inline]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub fn from_f64(v: f64) -> Result<Self, TypesError> {
        if v.is_nan() || v.is_infinite() || v < 0.0 {
            return Err(TypesError::SizeParse {
                input: v.to_string(),
            });
        }
        let scaled = (v * SIZE_SCALE as f64).round();
        // `as u64` saturates on overflow; reject instead of silently clamping
        // to u64::MAX.
        if scaled > u64::MAX as f64 {
            return Err(TypesError::SizeParse {
                input: v.to_string(),
            });
        }
        Ok(Self(scaled as u64))
    }

    #[inline]
    pub fn as_f64(self) -> f64 {
        self.0 as f64 / SIZE_SCALE as f64
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    #[inline]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for FixedSize {
    /// Exactly six fraction digits, e.g. `10.500000`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:06}", self.0 / SIZE_SCALE, self.0 % SIZE_SCALE)
    }
}

impl TryFrom<&str> for FixedSize {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let raw = parse_scaled_decimal(s, 6).ok_or_else(|| TypesError::SizeParse {
            input: s.to_string(),
        })?;
        // Reject sizes that overflow u64 instead of silently saturating.
        let raw: u64 = raw.try_into().map_err(|_| TypesError::SizeParse {
            input: s.to_string(),
        })?;
        Ok(Self(raw))
    }
}

/// See [`FixedPrice`]'s `Serialize` impl: decimal string for human-readable
/// formats, raw `u64` for binary ones.
impl Serialize for FixedSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.collect_str(self)
        } else {
            serializer.serialize_u64(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for FixedSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            deserializer.deserialize_str(FixedSizeVisitor)
        } else {
            u64::deserialize(deserializer).map(FixedSize)
        }
    }
}

struct FixedSizeVisitor;

impl<'de> serde::de::Visitor<'de> for FixedSizeVisitor {
    type Value = FixedSize;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a decimal string representing a non-negative size")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        FixedSize::try_from(v).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_price_roundtrip() {
        let p = FixedPrice::from_f64(0.5).unwrap();
        assert_eq!(p.raw(), 5000);
        assert!((p.as_f64() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_fixed_price_boundaries() {
        assert!(FixedPrice::new(0).is_ok());
        assert!(FixedPrice::new(10_000).is_ok());
        assert!(FixedPrice::new(10_001).is_err());
    }

    #[test]
    fn test_fixed_price_ordering() {
        let a = FixedPrice::from_f64(0.3).unwrap();
        let b = FixedPrice::from_f64(0.7).unwrap();
        assert!(a < b);
    }

    #[test]
    fn test_fixed_price_from_str() {
        let p = FixedPrice::try_from("0.1234").unwrap();
        assert_eq!(p.raw(), 1234);
    }

    #[test]
    fn test_fixed_price_serde() {
        let p = FixedPrice::from_f64(0.5).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"0.5000\"");
        let p2: FixedPrice = serde_json::from_str(&json).unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn test_fixed_size_roundtrip() {
        let s = FixedSize::from_f64(123.456789).unwrap();
        assert_eq!(s.raw(), 123_456_789);
        assert!((s.as_f64() - 123.456789).abs() < 1e-6);
    }

    #[test]
    fn test_fixed_size_from_str() {
        let s = FixedSize::try_from("100.5").unwrap();
        assert_eq!(s.raw(), 100_500_000);
    }

    #[test]
    fn test_fixed_size_serde() {
        let s = FixedSize::from_f64(10.0).unwrap();
        let json = serde_json::to_string(&s).unwrap();
        let s2: FixedSize = serde_json::from_str(&json).unwrap();
        assert_eq!(s, s2);
    }

    #[test]
    fn test_display() {
        let p = FixedPrice::from_f64(0.5).unwrap();
        assert_eq!(format!("{p}"), "0.5000");
        let s = FixedSize::from_f64(10.5).unwrap();
        assert_eq!(format!("{s}"), "10.500000");
    }

    #[test]
    fn test_fixed_price_rejects_nan() {
        assert!(FixedPrice::from_f64(f64::NAN).is_err());
    }

    #[test]
    fn test_fixed_price_rejects_negative() {
        assert!(FixedPrice::from_f64(-0.5).is_err());
    }

    #[test]
    fn test_fixed_price_rejects_infinity() {
        assert!(FixedPrice::from_f64(f64::INFINITY).is_err());
        assert!(FixedPrice::from_f64(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn test_fixed_size_rejects_nan() {
        assert!(FixedSize::from_f64(f64::NAN).is_err());
    }

    #[test]
    fn test_fixed_size_rejects_negative() {
        assert!(FixedSize::from_f64(-1.0).is_err());
    }

    #[test]
    fn test_fixed_size_rejects_infinity() {
        assert!(FixedSize::from_f64(f64::INFINITY).is_err());
        assert!(FixedSize::from_f64(f64::NEG_INFINITY).is_err());
    }

    // --- FixedPrice boundary / arithmetic tests ---

    #[test]
    fn fixed_price_new_at_exact_max() {
        let p = FixedPrice::new(10_000).unwrap();
        assert_eq!(p.raw(), 10_000);
        assert!((p.as_f64() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fixed_price_new_rejects_10001() {
        let err = FixedPrice::new(10_001).unwrap_err();
        match err {
            TypesError::InvalidPrice { raw } => assert_eq!(raw, 10_001),
            other => panic!("expected InvalidPrice, got: {other}"),
        }
    }

    #[test]
    fn fixed_price_new_rejects_u32_max() {
        assert!(FixedPrice::new(u32::MAX).is_err());
    }

    #[test]
    fn fixed_price_from_f64_at_scale_boundary() {
        // 0.9999 → raw 9999
        let p = FixedPrice::from_f64(0.9999).unwrap();
        assert_eq!(p.raw(), 9999);
        // 1.0000 → raw 10000
        let p = FixedPrice::from_f64(1.0).unwrap();
        assert_eq!(p.raw(), 10_000);
    }

    #[test]
    fn fixed_price_from_f64_rejects_just_over_1() {
        assert!(FixedPrice::from_f64(1.00005).is_err());
    }

    #[test]
    fn fixed_price_from_f64_zero() {
        let p = FixedPrice::from_f64(0.0).unwrap();
        assert_eq!(p.raw(), 0);
        assert!(p.is_zero());
    }

    #[test]
    fn fixed_price_display_leading_zeros() {
        let p = FixedPrice::new(10).unwrap(); // 0.0010
        assert_eq!(format!("{p}"), "0.0010");
    }

    #[test]
    fn fixed_price_try_from_invalid_str() {
        let err = FixedPrice::try_from("not_a_number").unwrap_err();
        match err {
            TypesError::PriceParse { input } => assert_eq!(input, "not_a_number"),
            other => panic!("expected PriceParse, got: {other}"),
        }
    }

    #[test]
    fn fixed_price_try_from_out_of_range() {
        assert!(FixedPrice::try_from("2.0").is_err());
    }

    #[test]
    fn fixed_price_serde_deserialize_rejects_invalid() {
        let result: Result<FixedPrice, _> = serde_json::from_str("\"abc\"");
        assert!(result.is_err());
    }

    #[test]
    fn fixed_price_serde_deserialize_rejects_out_of_range() {
        let result: Result<FixedPrice, _> = serde_json::from_str("\"5.0\"");
        assert!(result.is_err());
    }

    #[test]
    fn fixed_price_ordering_comprehensive() {
        let zero = FixedPrice::ZERO;
        let mid = FixedPrice::new(5000).unwrap();
        let one = FixedPrice::ONE;
        assert!(zero < mid);
        assert!(mid < one);
        assert!(zero < one);
        assert_eq!(mid, FixedPrice::new(5000).unwrap());
    }

    // --- FixedSize boundary / arithmetic tests ---

    #[test]
    fn fixed_size_new_at_u64_max() {
        let s = FixedSize::new(u64::MAX);
        assert_eq!(s.raw(), u64::MAX);
        assert!(!s.is_zero());
    }

    #[test]
    fn fixed_size_from_f64_zero() {
        let s = FixedSize::from_f64(0.0).unwrap();
        assert!(s.is_zero());
    }

    #[test]
    fn fixed_size_from_f64_very_small() {
        // Smallest representable size: 0.000001 → raw 1
        let s = FixedSize::from_f64(0.000001).unwrap();
        assert_eq!(s.raw(), 1);
    }

    #[test]
    fn fixed_size_display_leading_zeros() {
        let s = FixedSize::new(100); // 0.000100
        assert_eq!(format!("{s}"), "0.000100");
    }

    #[test]
    fn fixed_size_try_from_invalid_str() {
        let err = FixedSize::try_from("xyz").unwrap_err();
        match err {
            TypesError::SizeParse { input } => assert_eq!(input, "xyz"),
            other => panic!("expected SizeParse, got: {other}"),
        }
    }

    #[test]
    fn fixed_size_try_from_negative() {
        assert!(FixedSize::try_from("-1.0").is_err());
    }

    // --- Exact integer-decimal parsing (no f64 precision loss) ---

    #[test]
    fn fixed_size_parse_is_exact_above_f64_mantissa() {
        // 9_007_199_254_740_993 raw = 2^53 + 1, which f64 cannot represent
        // exactly. Integer parsing must round-trip it precisely.
        let raw = 9_007_199_254_740_993u64; // 9_007_199_254.740993 units
        let s = FixedSize::new(raw);
        let text = format!("{s}");
        let parsed = FixedSize::try_from(text.as_str()).unwrap();
        assert_eq!(
            parsed.raw(),
            raw,
            "exact integer parse must not lose precision"
        );
    }

    #[test]
    fn fixed_size_parse_rejects_excess_precision() {
        // 7 fractional digits exceeds the 6-decimal size scale.
        assert!(FixedSize::try_from("1.0000001").is_err());
    }

    #[test]
    fn fixed_price_parse_rejects_excess_precision() {
        // 5 fractional digits exceeds the 4-decimal price scale, and used to be
        // silently rounded.
        assert!(FixedPrice::try_from("0.12345").is_err());
    }

    #[test]
    fn fixed_size_parse_rejects_overflow_instead_of_saturating() {
        // A value far beyond u64::MAX must error, not saturate to u64::MAX.
        assert!(FixedSize::try_from("99999999999999999999.0").is_err());
    }

    #[test]
    fn fixed_size_parse_rejects_scientific_notation() {
        assert!(FixedSize::try_from("1e6").is_err());
    }

    #[test]
    fn fixed_price_parse_leading_dot_and_trailing_dot() {
        assert_eq!(FixedPrice::try_from(".5").unwrap().raw(), 5000);
        assert_eq!(FixedPrice::try_from("1.").unwrap().raw(), 10000);
    }

    #[test]
    fn fixed_size_serde_deserialize_rejects_invalid() {
        let result: Result<FixedSize, _> = serde_json::from_str("\"abc\"");
        assert!(result.is_err());
    }

    #[test]
    fn fixed_size_serde_deserialize_rejects_negative() {
        let result: Result<FixedSize, _> = serde_json::from_str("\"-1.0\"");
        assert!(result.is_err());
    }

    #[test]
    fn fixed_size_ordering_comprehensive() {
        let zero = FixedSize::ZERO;
        let small = FixedSize::new(1);
        let big = FixedSize::new(u64::MAX);
        assert!(zero < small);
        assert!(small < big);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn fixed_price_raw_roundtrip(raw in 0u32..=PRICE_SCALE) {
            let p = FixedPrice::new(raw).unwrap();
            prop_assert_eq!(p.raw(), raw);
        }

        #[test]
        fn fixed_price_f64_roundtrip(raw in 0u32..=PRICE_SCALE) {
            let p = FixedPrice::new(raw).unwrap();
            let f = p.as_f64();
            let p2 = FixedPrice::from_f64(f).unwrap();
            prop_assert_eq!(p, p2);
        }

        #[test]
        fn fixed_price_ordering_preserved(a_raw in 0u32..=PRICE_SCALE, b_raw in 0u32..=PRICE_SCALE) {
            let a = FixedPrice::new(a_raw).unwrap();
            let b = FixedPrice::new(b_raw).unwrap();
            prop_assert_eq!(a.cmp(&b), a_raw.cmp(&b_raw));
        }

        #[test]
        fn fixed_price_rejects_out_of_range(raw in (PRICE_SCALE + 1)..=u32::MAX) {
            prop_assert!(FixedPrice::new(raw).is_err());
        }

        #[test]
        fn fixed_price_serde_roundtrip(raw in 0u32..=PRICE_SCALE) {
            let p = FixedPrice::new(raw).unwrap();
            let json = serde_json::to_string(&p).unwrap();
            let p2: FixedPrice = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(p, p2);
        }

        #[test]
        fn fixed_size_raw_roundtrip(raw in 0u64..=10_000_000_000u64) {
            let s = FixedSize::new(raw);
            prop_assert_eq!(s.raw(), raw);
        }

        #[test]
        fn fixed_size_f64_roundtrip(raw in 0u64..=10_000_000u64) {
            let s = FixedSize::new(raw);
            let f = s.as_f64();
            let s2 = FixedSize::from_f64(f).unwrap();
            prop_assert_eq!(s, s2);
        }

        #[test]
        fn fixed_size_ordering_preserved(a_raw in 0u64..=u64::MAX, b_raw in 0u64..=u64::MAX) {
            let a = FixedSize::new(a_raw);
            let b = FixedSize::new(b_raw);
            prop_assert_eq!(a.cmp(&b), a_raw.cmp(&b_raw));
        }

        #[test]
        fn fixed_size_zero_detection(raw in 0u64..=10_000_000_000u64) {
            let s = FixedSize::new(raw);
            prop_assert_eq!(s.is_zero(), raw == 0);
        }

        /// FixedPrice Display then parse roundtrips correctly.
        #[test]
        fn fixed_price_display_parse_roundtrip(raw in 0u32..=PRICE_SCALE) {
            let p = FixedPrice::new(raw).unwrap();
            let s = format!("{p}");
            let p2 = FixedPrice::try_from(s.as_str()).unwrap();
            prop_assert_eq!(p, p2);
        }

        /// FixedSize Display then parse roundtrips correctly.
        #[test]
        fn fixed_size_display_parse_roundtrip(raw in 0u64..=10_000_000_000u64) {
            let s = FixedSize::new(raw);
            let display = format!("{s}");
            let s2 = FixedSize::try_from(display.as_str()).unwrap();
            prop_assert_eq!(s, s2);
        }

        /// Exact Display→parse roundtrip across the ENTIRE u64 range, including
        /// values above 2^53 that f64 cannot represent. This is the property the
        /// old f64-based parser violated: the WAL codec and checkpoint
        /// JSON both round-trip sizes through this path.
        #[test]
        fn fixed_size_display_parse_roundtrip_full_u64(raw in 0u64..=u64::MAX) {
            let s = FixedSize::new(raw);
            let display = format!("{s}");
            let s2 = FixedSize::try_from(display.as_str()).unwrap();
            prop_assert_eq!(s.raw(), s2.raw());
        }

        /// FixedPrice serde roundtrip for all valid raws including boundary values.
        #[test]
        fn fixed_price_serde_boundary_roundtrip(raw in proptest::prop_oneof![
            Just(0u32),
            Just(1u32),
            Just(9999u32),
            Just(10_000u32),
            0u32..=PRICE_SCALE,
        ]) {
            let p = FixedPrice::new(raw).unwrap();
            let json = serde_json::to_string(&p).unwrap();
            let p2: FixedPrice = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(p, p2);
        }

        /// FixedSize serde roundtrip at large values.
        #[test]
        fn fixed_size_serde_large_roundtrip(raw in 0u64..=100_000_000_000u64) {
            let s = FixedSize::new(raw);
            let json = serde_json::to_string(&s).unwrap();
            let s2: FixedSize = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(s, s2);
        }

        /// FixedPrice is_zero is correct for all valid values.
        #[test]
        fn fixed_price_is_zero_correct(raw in 0u32..=PRICE_SCALE) {
            let p = FixedPrice::new(raw).unwrap();
            prop_assert_eq!(p.is_zero(), raw == 0);
        }

        /// FixedPrice::from_f64 and new produce same result for valid range.
        #[test]
        fn fixed_price_from_f64_matches_new(raw in 0u32..=PRICE_SCALE) {
            let p1 = FixedPrice::new(raw).unwrap();
            let f = raw as f64 / PRICE_SCALE as f64;
            let p2 = FixedPrice::from_f64(f).unwrap();
            prop_assert_eq!(p1, p2);
        }
    }

    #[test]
    fn binary_formats_carry_raw_integers_and_json_carries_decimals() {
        let p = FixedPrice::new(5000).unwrap();
        let s = FixedSize::new(10_500_000);
        // bincode is not human-readable: 4 + 8 bytes, no strings.
        let bin = bincode::serialize(&(p, s)).unwrap();
        assert_eq!(bin.len(), 12);
        let (p2, s2): (FixedPrice, FixedSize) = bincode::deserialize(&bin).unwrap();
        assert_eq!((p, s), (p2, s2));
        // An out-of-range raw price is rejected on the binary path too.
        assert!(bincode::deserialize::<FixedPrice>(&10_001u32.to_le_bytes()).is_err());
        // JSON keeps the exact decimal strings.
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"0.5000\"");
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"10.500000\"");
    }
}
