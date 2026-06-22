use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Polymarket token ID (condition_id).
/// Uses `Arc<str>` so `.clone()` is a cheap ref-count bump.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub Arc<str>);

impl AssetId {
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for AssetId {
    fn from(s: String) -> Self {
        Self(Arc::from(s))
    }
}

impl From<&str> for AssetId {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

/// Monotonically increasing sequence number for gap detection.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Sequence(pub u64);

impl Sequence {
    #[inline]
    pub const fn new(seq: u64) -> Self {
        Self(seq)
    }

    #[inline]
    pub const fn next(self) -> Self {
        // wrapping_add, not `+ 1`: with overflow-checks = true a plain `+ 1`
        // panics at u64::MAX. Sequence is a wrap-aware counter, so wrapping is the
        // correct boundary behavior and removes the panic.
        Self(self.0.wrapping_add(1))
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Sequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_id() {
        let id = AssetId::new("abc123");
        assert_eq!(id.as_str(), "abc123");
        assert_eq!(format!("{id}"), "abc123");
    }

    #[test]
    fn test_sequence_ordering() {
        let a = Sequence::new(1);
        let b = Sequence::new(2);
        assert!(a < b);
        assert_eq!(a.next(), b);
    }

    // --- Sequence hardened tests ---

    #[test]
    fn sequence_zero_default() {
        assert_eq!(Sequence::default(), Sequence::new(0));
        assert_eq!(Sequence::default().raw(), 0);
    }

    #[test]
    fn sequence_next_from_zero() {
        assert_eq!(Sequence::new(0).next(), Sequence::new(1));
    }

    #[test]
    fn sequence_const_fn_in_const_context() {
        const SEQ: Sequence = Sequence::new(42);
        const RAW: u64 = SEQ.raw();
        const NEXT: Sequence = SEQ.next();
        assert_eq!(RAW, 42);
        assert_eq!(NEXT.raw(), 43);
    }

    #[test]
    fn sequence_display() {
        assert_eq!(format!("{}", Sequence::new(0)), "0");
        assert_eq!(
            format!("{}", Sequence::new(u64::MAX)),
            format!("{}", u64::MAX)
        );
    }

    #[test]
    fn sequence_ordering_comprehensive() {
        let a = Sequence::new(0);
        let b = Sequence::new(1);
        let c = Sequence::new(u64::MAX);
        assert!(a < b);
        assert!(b < c);
        assert!(a < c);
        assert_eq!(a, Sequence::new(0));
    }

    #[test]
    fn sequence_serde_roundtrip() {
        let seq = Sequence::new(999_999);
        let json = serde_json::to_string(&seq).unwrap();
        let seq2: Sequence = serde_json::from_str(&json).unwrap();
        assert_eq!(seq, seq2);
    }

    #[test]
    fn sequence_serde_zero() {
        let seq = Sequence::new(0);
        let json = serde_json::to_string(&seq).unwrap();
        assert_eq!(json, "0");
        let seq2: Sequence = serde_json::from_str(&json).unwrap();
        assert_eq!(seq, seq2);
    }

    #[test]
    fn sequence_serde_u64_max() {
        let seq = Sequence::new(u64::MAX);
        let json = serde_json::to_string(&seq).unwrap();
        let seq2: Sequence = serde_json::from_str(&json).unwrap();
        assert_eq!(seq, seq2);
    }

    #[test]
    fn sequence_hash_equality() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Sequence::new(10));
        assert!(set.contains(&Sequence::new(10)));
        assert!(!set.contains(&Sequence::new(11)));
    }

    #[test]
    fn sequence_copy_semantics() {
        let s = Sequence::new(100);
        let s2 = s;
        assert_eq!(s, s2);
    }

    // --- AssetId hardened tests ---

    #[test]
    fn asset_id_from_string() {
        let id = AssetId::from("hello".to_string());
        assert_eq!(id.as_str(), "hello");
    }

    #[test]
    fn asset_id_from_str_ref() {
        let id = AssetId::from("world");
        assert_eq!(id.as_str(), "world");
    }

    #[test]
    fn asset_id_clone_is_cheap() {
        let id = AssetId::new("shared-data");
        let cloned = id.clone();
        // Arc-based clone, both point to same allocation
        assert_eq!(id, cloned);
        assert_eq!(id.as_str(), cloned.as_str());
    }

    #[test]
    fn asset_id_empty_string() {
        let id = AssetId::new("");
        assert_eq!(id.as_str(), "");
        assert_eq!(format!("{id}"), "");
    }

    #[test]
    fn asset_id_long_token_id() {
        let long = "21742633143463801764263866138596936600980228888098934498299596572218858267895";
        let id = AssetId::new(long);
        assert_eq!(id.as_str(), long);
    }

    #[test]
    fn asset_id_serde_roundtrip() {
        let id = AssetId::new("token-123");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"token-123\"");
        let id2: AssetId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn asset_id_hash_equality() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(AssetId::new("abc"));
        assert!(set.contains(&AssetId::new("abc")));
        assert!(!set.contains(&AssetId::new("def")));
    }

    #[test]
    fn asset_id_equality_independent_of_construction() {
        let a = AssetId::new("test");
        let b = AssetId::from("test".to_string());
        let c = AssetId::from("test");
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn sequence_next_wraps_at_max() {
        // wrapping_add: with overflow-checks = true a plain `+ 1` would panic at
        // u64::MAX. Sequence is a wrap-aware counter.
        assert_eq!(Sequence::new(u64::MAX).next().raw(), 0);
        assert_eq!(Sequence::new(5).next().raw(), 6);
    }
}
