use serde::{Deserialize, Serialize};
use std::fmt;

/// Polymarket token ID (condition_id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub String);

impl AssetId {
    pub fn new(id: impl Into<String>) -> Self {
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
        Self(s)
    }
}

impl From<&str> for AssetId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Monotonically increasing sequence number for gap detection.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Sequence(pub u64);

impl Sequence {
    pub fn new(seq: u64) -> Self {
        Self(seq)
    }

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    pub fn raw(self) -> u64 {
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
}
