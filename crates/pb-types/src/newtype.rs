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

    /// Filesystem/object-store safe key for partition filenames.
    pub fn storage_key(&self) -> String {
        storage_key_for(self.as_str())
    }
}

/// Percent-encode an asset partition for use in object-store filenames.
///
/// Keeps common safe ASCII unchanged for readable paths and encodes every other
/// byte as `%XX`, including `/`, `\`, control bytes, `%`, and non-ASCII.
pub fn storage_key_for(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Extract the encoded asset component from the current Parquet object filename
/// format: `{asset}_{first_ts_us}_{content_hash}_{len}.parquet`.
///
/// The asset key itself may contain `_`, so callers must split from the right
/// and validate the fixed suffix fields instead of using `starts_with`.
pub fn storage_file_asset_key(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".parquet")?;
    let mut parts = stem.rsplitn(4, '_');
    let len = parts.next()?;
    let hash = parts.next()?;
    let first_ts_us = parts.next()?;
    let asset = parts.next()?;

    if asset.is_empty()
        || first_ts_us.is_empty()
        || len.is_empty()
        || !first_ts_us.bytes().all(|byte| byte.is_ascii_digit())
        || !len.bytes().all(|byte| byte.is_ascii_digit())
        || hash.len() != 16
        || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }

    Some(asset)
}

pub fn storage_file_matches_asset(file_name: &str, asset_key: &str) -> bool {
    storage_file_asset_key(file_name) == Some(asset_key)
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
    fn storage_key_percent_encodes_path_separators_and_percent() {
        assert_eq!(storage_key_for("abc-123_DEF.4"), "abc-123_DEF.4");
        assert_eq!(storage_key_for("../a/b%"), "..%2Fa%2Fb%25");
        assert_eq!(AssetId::new("a\\b").storage_key(), "a%5Cb");
    }

    #[test]
    fn storage_file_asset_match_is_exact_with_underscore_asset_ids() {
        let foo = "foo_1700000000000000_0123456789abcdef_42.parquet";
        let foo_bar = "foo_bar_1700000000000000_0123456789abcdef_42.parquet";

        assert_eq!(storage_file_asset_key(foo), Some("foo"));
        assert_eq!(storage_file_asset_key(foo_bar), Some("foo_bar"));
        assert!(storage_file_matches_asset(foo, "foo"));
        assert!(storage_file_matches_asset(foo_bar, "foo_bar"));
        assert!(!storage_file_matches_asset(foo_bar, "foo"));
        assert!(!storage_file_matches_asset(foo, "foo_bar"));
        assert!(!storage_file_matches_asset("foo_bar.parquet", "foo"));
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
