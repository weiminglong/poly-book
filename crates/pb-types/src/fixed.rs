use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

use crate::error::TypesError;

const PRICE_SCALE: u32 = 10_000;
const SIZE_SCALE: u64 = 1_000_000;

/// Fixed-point price representation: value * 10,000.
/// Polymarket prices are 0.00–1.00, so range is 0–10,000.
/// 4 bytes, `Copy`, trivial `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FixedPrice(pub u32);

impl FixedPrice {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(PRICE_SCALE);
    pub const SCALE: u32 = PRICE_SCALE;

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

    pub fn as_f64(self) -> f64 {
        self.0 as f64 / PRICE_SCALE as f64
    }

    pub fn raw(self) -> u32 {
        self.0
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for FixedPrice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.4}", self.as_f64())
    }
}

impl TryFrom<&str> for FixedPrice {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let v: f64 = s.parse().map_err(|_| TypesError::PriceParse {
            input: s.to_string(),
        })?;
        Self::from_f64(v)
    }
}

impl Serialize for FixedPrice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:.4}", self.as_f64()))
    }
}

impl<'de> Deserialize<'de> for FixedPrice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        FixedPrice::try_from(s.as_str()).map_err(serde::de::Error::custom)
    }
}

/// Fixed-point size representation: value * 1,000,000.
/// 8 bytes, `Copy`, trivial `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FixedSize(pub u64);

impl FixedSize {
    pub const ZERO: Self = Self(0);
    pub const SCALE: u64 = SIZE_SCALE;

    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub fn from_f64(v: f64) -> Result<Self, TypesError> {
        if v.is_nan() || v.is_infinite() || v < 0.0 {
            return Err(TypesError::SizeParse {
                input: v.to_string(),
            });
        }
        Ok(Self((v * SIZE_SCALE as f64).round() as u64))
    }

    pub fn as_f64(self) -> f64 {
        self.0 as f64 / SIZE_SCALE as f64
    }

    pub fn raw(self) -> u64 {
        self.0
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for FixedSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6}", self.as_f64())
    }
}

impl TryFrom<&str> for FixedSize {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let v: f64 = s.parse().map_err(|_| TypesError::SizeParse {
            input: s.to_string(),
        })?;
        Self::from_f64(v)
    }
}

impl Serialize for FixedSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:.6}", self.as_f64()))
    }
}

impl<'de> Deserialize<'de> for FixedSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        FixedSize::try_from(s.as_str()).map_err(serde::de::Error::custom)
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
    }
}
