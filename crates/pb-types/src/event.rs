use serde::{Deserialize, Serialize};

use crate::fixed::{FixedPrice, FixedSize};
use crate::newtype::{AssetId, Sequence};

/// Bid or Ask side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Bid,
    Ask,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Bid => write!(f, "Bid"),
            Side::Ask => write!(f, "Ask"),
        }
    }
}

/// Source system for a persisted record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataSource {
    WebSocket,
    RestSnapshot,
    ReplayValidator,
    System,
}

impl std::fmt::Display for DataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataSource::WebSocket => write!(f, "websocket"),
            DataSource::RestSnapshot => write!(f, "rest_snapshot"),
            DataSource::ReplayValidator => write!(f, "replay_validator"),
            DataSource::System => write!(f, "system"),
        }
    }
}

/// Shared provenance captured for persisted records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventProvenance {
    pub recv_timestamp_us: u64,
    pub exchange_timestamp_us: u64,
    pub source: DataSource,
    pub source_event_id: Option<String>,
    pub source_session_id: Option<String>,
    pub sequence: Option<Sequence>,
    /// Process-monotonic ordinal stamped at the single ingest serialization
    /// point, strictly increasing in true arrival order across snapshots and
    /// reconnects (unlike `sequence`, which resets to 0 on every snapshot). This
    /// is the authoritative replay tiebreaker so a same-microsecond pre-snapshot
    /// delta sorts before its snapshot. `None` for records
    /// produced before this field existed or outside the ingest path (e.g. replay
    /// reconstructs `IngestEvent`s for surfaced gaps).
    #[serde(default)]
    pub ingest_ordinal: Option<u64>,
}

/// Book event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BookEventKind {
    Snapshot,
    Delta,
}

impl std::fmt::Display for BookEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BookEventKind::Snapshot => write!(f, "Snapshot"),
            BookEventKind::Delta => write!(f, "Delta"),
        }
    }
}

/// Trade fidelity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TradeFidelity {
    Partial,
    Full,
}

impl std::fmt::Display for TradeFidelity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeFidelity::Partial => write!(f, "partial"),
            TradeFidelity::Full => write!(f, "full"),
        }
    }
}

/// Persisted book event used for reconstruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookEvent {
    pub asset_id: AssetId,
    pub kind: BookEventKind,
    pub side: Side,
    pub price: FixedPrice,
    pub size: FixedSize,
    pub provenance: EventProvenance,
}

/// Persisted trade event used for trade-aware analytics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeEvent {
    pub asset_id: AssetId,
    pub price: FixedPrice,
    pub size: Option<FixedSize>,
    pub side: Option<Side>,
    pub trade_id: Option<String>,
    pub fidelity: TradeFidelity,
    pub provenance: EventProvenance,
}

/// Ingest lifecycle event used to explain continuity boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IngestEventKind {
    ReconnectStart,
    ReconnectSuccess,
    SequenceGap,
    StaleSnapshotSkip,
    SourceReset,
    /// The reconstructed top-of-book diverged from the venue-stated
    /// `best_bid`/`best_ask` after applying a delta — evidence of a silently
    /// dropped/corrupt update.
    BookMismatch,
}

impl std::fmt::Display for IngestEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestEventKind::ReconnectStart => write!(f, "reconnect_start"),
            IngestEventKind::ReconnectSuccess => write!(f, "reconnect_success"),
            IngestEventKind::SequenceGap => write!(f, "sequence_gap"),
            IngestEventKind::StaleSnapshotSkip => write!(f, "stale_snapshot_skip"),
            IngestEventKind::SourceReset => write!(f, "source_reset"),
            IngestEventKind::BookMismatch => write!(f, "book_mismatch"),
        }
    }
}

impl IngestEventKind {
    /// Returns true when the event represents a hard continuity reset between
    /// feed sessions. Replay should not stitch state across this boundary.
    pub fn is_continuity_reset(self) -> bool {
        matches!(self, Self::SourceReset)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestEvent {
    pub asset_id: Option<AssetId>,
    pub kind: IngestEventKind,
    pub provenance: EventProvenance,
    pub expected_sequence: Option<u64>,
    pub observed_sequence: Option<u64>,
    pub details: Option<String>,
}

/// A price-size level used in snapshots and checkpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: FixedPrice,
    pub size: FixedSize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookCheckpoint {
    pub asset_id: AssetId,
    pub checkpoint_timestamp_us: u64,
    pub provenance: EventProvenance,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    /// WAL global byte offset at the time this checkpoint was produced.
    /// Used by checkpoint hydration to resume WAL tailing from this position.
    #[serde(default)]
    pub wal_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReplayMode {
    RecvTime,
    ExchangeTime,
}

impl std::fmt::Display for ReplayMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayMode::RecvTime => write!(f, "recv_time"),
            ReplayMode::ExchangeTime => write!(f, "exchange_time"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayValidation {
    pub asset_id: AssetId,
    pub mode: ReplayMode,
    pub replay_timestamp_us: u64,
    pub reference_timestamp_us: u64,
    pub matched: bool,
    pub mismatch_summary: Option<String>,
    pub persisted_at_us: u64,
}

impl std::str::FromStr for Side {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bid" | "Bid" => Ok(Self::Bid),
            "ask" | "Ask" => Ok(Self::Ask),
            other => Err(format!("unknown side: {other}")),
        }
    }
}

/// Persisted record routed through storage sinks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedRecord {
    Book(BookEvent),
    Trade(TradeEvent),
    Ingest(IngestEvent),
    Checkpoint(BookCheckpoint),
    Validation(ReplayValidation),
}

impl PersistedRecord {
    pub fn dataset_name(&self) -> &'static str {
        match self {
            PersistedRecord::Book(_) => "book_events",
            PersistedRecord::Trade(_) => "trade_events",
            PersistedRecord::Ingest(_) => "ingest_events",
            PersistedRecord::Checkpoint(_) => "book_checkpoints",
            PersistedRecord::Validation(_) => "replay_validations",
        }
    }

    pub fn asset_partition(&self) -> &str {
        match self {
            PersistedRecord::Book(event) => event.asset_id.as_str(),
            PersistedRecord::Trade(event) => event.asset_id.as_str(),
            PersistedRecord::Ingest(event) => event
                .asset_id
                .as_ref()
                .map(|id| id.as_str())
                .unwrap_or("global"),
            PersistedRecord::Checkpoint(event) => event.asset_id.as_str(),
            PersistedRecord::Validation(event) => event.asset_id.as_str(),
        }
    }

    pub fn partition_timestamp_us(&self) -> u64 {
        match self {
            PersistedRecord::Book(event) => event.provenance.recv_timestamp_us,
            PersistedRecord::Trade(event) => event.provenance.recv_timestamp_us,
            PersistedRecord::Ingest(event) => event.provenance.recv_timestamp_us,
            PersistedRecord::Checkpoint(event) => event.checkpoint_timestamp_us,
            PersistedRecord::Validation(event) => event.persisted_at_us,
        }
    }

    /// The feed-receive timestamp (µs) for records that carry provenance, used to
    /// measure end-to-end recv→durable latency at ingest.
    /// `ReplayValidation` has no feed-receive provenance.
    pub fn recv_timestamp_us(&self) -> Option<u64> {
        match self {
            PersistedRecord::Book(e) => Some(e.provenance.recv_timestamp_us),
            PersistedRecord::Trade(e) => Some(e.provenance.recv_timestamp_us),
            PersistedRecord::Ingest(e) => Some(e.provenance.recv_timestamp_us),
            PersistedRecord::Checkpoint(e) => Some(e.provenance.recv_timestamp_us),
            PersistedRecord::Validation(_) => None,
        }
    }

    /// Mutable access to the record's `EventProvenance`, if it carries one.
    /// `ReplayValidation` has no provenance and returns `None`. Used at the single ingest serialization point to stamp the
    /// monotonic `ingest_ordinal`.
    pub fn provenance_mut(&mut self) -> Option<&mut EventProvenance> {
        match self {
            PersistedRecord::Book(event) => Some(&mut event.provenance),
            PersistedRecord::Trade(event) => Some(&mut event.provenance),
            PersistedRecord::Ingest(event) => Some(&mut event.provenance),
            PersistedRecord::Checkpoint(event) => Some(&mut event.provenance),
            PersistedRecord::Validation(_) => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketDataWindow {
    pub book_events: Vec<BookEvent>,
    pub trade_events: Vec<TradeEvent>,
    pub ingest_events: Vec<IngestEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_provenance() -> EventProvenance {
        EventProvenance {
            recv_timestamp_us: 1_000_000,
            exchange_timestamp_us: 999_000,
            source: DataSource::WebSocket,
            source_event_id: Some("abc".to_string()),
            source_session_id: Some("session-1".to_string()),
            sequence: Some(Sequence::new(42)),
            ingest_ordinal: None,
        }
    }

    #[test]
    fn test_dataset_name() {
        let record = PersistedRecord::Ingest(IngestEvent {
            asset_id: None,
            kind: IngestEventKind::ReconnectStart,
            provenance: sample_provenance(),
            expected_sequence: None,
            observed_sequence: None,
            details: None,
        });
        assert_eq!(record.dataset_name(), "ingest_events");
        assert_eq!(record.asset_partition(), "global");
    }

    #[test]
    fn side_from_str() {
        assert_eq!("Bid".parse::<Side>().unwrap(), Side::Bid);
        assert_eq!("bid".parse::<Side>().unwrap(), Side::Bid);
        assert_eq!("Ask".parse::<Side>().unwrap(), Side::Ask);
        assert_eq!("ask".parse::<Side>().unwrap(), Side::Ask);
        assert!("BUY".parse::<Side>().is_err());
        assert!("".parse::<Side>().is_err());
    }

    #[test]
    fn book_checkpoint_wal_offset_default() {
        // wal_offset has #[serde(default)] so it should be None when missing
        let json = r#"{
            "asset_id": "tok-6",
            "checkpoint_timestamp_us": 0,
            "provenance": {
                "recv_timestamp_us": 0,
                "exchange_timestamp_us": 0,
                "source": "WebSocket",
                "source_event_id": null,
                "source_session_id": null,
                "sequence": null
            },
            "bids": [],
            "asks": []
        }"#;
        let cp: BookCheckpoint = serde_json::from_str(json).unwrap();
        assert_eq!(cp.wal_offset, None);
    }

    // --- PersistedRecord tests ---

    #[test]
    fn persisted_record_dataset_names_all_variants() {
        let prov = sample_provenance();
        let records = [
            (
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("a"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Bid,
                    price: FixedPrice::ZERO,
                    size: FixedSize::ZERO,
                    provenance: prov.clone(),
                }),
                "book_events",
            ),
            (
                PersistedRecord::Trade(TradeEvent {
                    asset_id: AssetId::new("a"),
                    price: FixedPrice::ZERO,
                    size: None,
                    side: None,
                    trade_id: None,
                    fidelity: TradeFidelity::Partial,
                    provenance: prov.clone(),
                }),
                "trade_events",
            ),
            (
                PersistedRecord::Ingest(IngestEvent {
                    asset_id: None,
                    kind: IngestEventKind::ReconnectStart,
                    provenance: prov.clone(),
                    expected_sequence: None,
                    observed_sequence: None,
                    details: None,
                }),
                "ingest_events",
            ),
            (
                PersistedRecord::Checkpoint(BookCheckpoint {
                    asset_id: AssetId::new("a"),
                    checkpoint_timestamp_us: 0,
                    provenance: prov.clone(),
                    bids: vec![],
                    asks: vec![],
                    wal_offset: None,
                }),
                "book_checkpoints",
            ),
            (
                PersistedRecord::Validation(ReplayValidation {
                    asset_id: AssetId::new("a"),
                    mode: ReplayMode::RecvTime,
                    replay_timestamp_us: 0,
                    reference_timestamp_us: 0,
                    matched: true,
                    mismatch_summary: None,
                    persisted_at_us: 0,
                }),
                "replay_validations",
            ),
        ];
        for (record, expected_name) in &records {
            assert_eq!(record.dataset_name(), *expected_name);
        }
    }

    #[test]
    fn persisted_record_asset_partition_with_asset() {
        let prov = sample_provenance();
        let record = PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new("my-token"),
            kind: BookEventKind::Delta,
            side: Side::Ask,
            price: FixedPrice::from_f64(0.5).unwrap(),
            size: FixedSize::new(1_000_000),
            provenance: prov,
        });
        assert_eq!(record.asset_partition(), "my-token");
    }

    #[test]
    fn persisted_record_asset_partition_global_ingest() {
        let prov = sample_provenance();
        let record = PersistedRecord::Ingest(IngestEvent {
            asset_id: None,
            kind: IngestEventKind::ReconnectStart,
            provenance: prov,
            expected_sequence: None,
            observed_sequence: None,
            details: None,
        });
        assert_eq!(record.asset_partition(), "global");
    }

    #[test]
    fn persisted_record_partition_timestamp_all_variants() {
        let prov = EventProvenance {
            recv_timestamp_us: 111,
            exchange_timestamp_us: 222,
            source: DataSource::WebSocket,
            source_event_id: None,
            source_session_id: None,
            sequence: None,
            ingest_ordinal: None,
        };

        // Book uses provenance recv_timestamp_us
        let r = PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new("a"),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::ZERO,
            size: FixedSize::ZERO,
            provenance: prov.clone(),
        });
        assert_eq!(r.partition_timestamp_us(), 111);

        // Checkpoint uses checkpoint_timestamp_us
        let r = PersistedRecord::Checkpoint(BookCheckpoint {
            asset_id: AssetId::new("a"),
            checkpoint_timestamp_us: 333,
            provenance: prov.clone(),
            bids: vec![],
            asks: vec![],
            wal_offset: None,
        });
        assert_eq!(r.partition_timestamp_us(), 333);

        // Validation uses persisted_at_us
        let r = PersistedRecord::Validation(ReplayValidation {
            asset_id: AssetId::new("a"),
            mode: ReplayMode::RecvTime,
            replay_timestamp_us: 0,
            reference_timestamp_us: 0,
            matched: true,
            mismatch_summary: None,
            persisted_at_us: 444,
        });
        assert_eq!(r.partition_timestamp_us(), 444);
    }
}
