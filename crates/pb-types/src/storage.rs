//! Shared storage-layout metadata used by Parquet writers and readers.

use serde::{Deserialize, Serialize};

/// Current format version for crash-consistent Parquet recovery manifests.
pub const PARQUET_RECOVERY_MANIFEST_VERSION: u32 = 1;

/// Hidden object-store prefix containing the authoritative recovery manifests.
pub const PARQUET_RECOVERY_MANIFEST_PREFIX: &str = "_recovery_manifests";

/// Hidden staging prefix used during two-phase recovery publication. A staged
/// object may temporarily be authoritative between the staged and final
/// manifest cuts; successful recovery promotes it to the normal partition and
/// removes the hidden object.
pub const PARQUET_RECOVERY_OBJECT_PREFIX: &str = "_recovery_objects";

/// Atomically published view of one recovered `(dataset, asset, hour)` partition.
///
/// Recovery publishes a staged generation and then a promoted normal-tree
/// generation through this small manifest. Readers switch views at each
/// manifest replacement. `superseded_objects` keeps old files invisible when
/// cleanup has not completed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParquetRecoveryManifest {
    pub version: u32,
    pub dataset: String,
    pub asset_key: String,
    pub hour_key: String,
    pub covered_start_us: u64,
    pub covered_end_us: u64,
    pub active_objects: Vec<String>,
    pub superseded_objects: Vec<String>,
}

impl ParquetRecoveryManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != PARQUET_RECOVERY_MANIFEST_VERSION {
            return Err(format!(
                "unsupported recovery manifest version {}, expected {}",
                self.version, PARQUET_RECOVERY_MANIFEST_VERSION
            ));
        }
        if self.dataset.is_empty() || self.asset_key.is_empty() || self.hour_key.is_empty() {
            return Err("recovery manifest identity fields must not be empty".to_string());
        }
        if self.covered_start_us >= self.covered_end_us {
            return Err("recovery manifest coverage must be an increasing range".to_string());
        }
        if self.active_objects.is_empty() {
            return Err("recovery manifest must name at least one active object".to_string());
        }
        if self
            .active_objects
            .iter()
            .any(|active| self.superseded_objects.contains(active))
        {
            return Err("recovery manifest cannot supersede an active object".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manifest() -> ParquetRecoveryManifest {
        ParquetRecoveryManifest {
            version: PARQUET_RECOVERY_MANIFEST_VERSION,
            dataset: "book_events".to_string(),
            asset_key: "token-1".to_string(),
            hour_key: "2026/07/21/10".to_string(),
            covered_start_us: 1_774_000_000_000_000,
            covered_end_us: 1_774_003_600_000_000,
            active_objects: vec!["data/_recovery_objects/book.parquet".to_string()],
            superseded_objects: vec!["data/book_events/old.parquet".to_string()],
        }
    }

    #[test]
    fn recovery_manifest_accepts_current_complete_shape() {
        assert!(valid_manifest().validate().is_ok());
    }

    #[test]
    fn recovery_manifest_rejects_unknown_version_and_empty_active_set() {
        let mut manifest = valid_manifest();
        manifest.version += 1;
        assert!(manifest.validate().is_err());

        let mut manifest = valid_manifest();
        manifest.active_objects.clear();
        assert!(manifest.validate().is_err());
    }
}
