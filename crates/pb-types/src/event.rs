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
    Strategy,
    Exchange,
    System,
}

impl std::fmt::Display for DataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataSource::WebSocket => write!(f, "websocket"),
            DataSource::RestSnapshot => write!(f, "rest_snapshot"),
            DataSource::ReplayValidator => write!(f, "replay_validator"),
            DataSource::Strategy => write!(f, "strategy"),
            DataSource::Exchange => write!(f, "exchange"),
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
    /// delta sorts before its snapshot (audit finding A.116). `None` for records
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
    /// dropped/corrupt update (audit findings A.74/A.109).
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LatencyTrace {
    pub market_data_recv_us: Option<u64>,
    pub normalization_done_us: Option<u64>,
    pub strategy_decision_us: Option<u64>,
    pub order_submit_us: Option<u64>,
    pub exchange_ack_us: Option<u64>,
    pub exchange_fill_us: Option<u64>,
}

impl LatencyTrace {
    pub fn from_optional_timestamps(
        market_data_recv_us: Option<u64>,
        normalization_done_us: Option<u64>,
        strategy_decision_us: Option<u64>,
        order_submit_us: Option<u64>,
        exchange_ack_us: Option<u64>,
        exchange_fill_us: Option<u64>,
    ) -> Self {
        Self {
            market_data_recv_us,
            normalization_done_us,
            strategy_decision_us,
            order_submit_us,
            exchange_ack_us,
            exchange_fill_us,
        }
    }

    /// The latency stages in causal order, skipping absent ones.
    fn present_stages(&self) -> impl Iterator<Item = u64> + '_ {
        [
            self.market_data_recv_us,
            self.normalization_done_us,
            self.strategy_decision_us,
            self.order_submit_us,
            self.exchange_ack_us,
            self.exchange_fill_us,
        ]
        .into_iter()
        .flatten()
    }

    /// True if the present stage timestamps are non-decreasing in causal order.
    /// A violation means a consumer would compute a negative stage duration
    /// (audit finding A.62).
    pub fn is_monotonic(&self) -> bool {
        let mut last: Option<u64> = None;
        for stage in self.present_stages() {
            if last.is_some_and(|prev| stage < prev) {
                return false;
            }
            last = Some(stage);
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionEventKind {
    SubmitIntent,
    ExchangeAck,
    CancelRequest,
    CancelAck,
    Reject,
    PartialFill,
    Fill,
    Terminal,
}

impl std::fmt::Display for ExecutionEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionEventKind::SubmitIntent => write!(f, "submit_intent"),
            ExecutionEventKind::ExchangeAck => write!(f, "exchange_ack"),
            ExecutionEventKind::CancelRequest => write!(f, "cancel_request"),
            ExecutionEventKind::CancelAck => write!(f, "cancel_ack"),
            ExecutionEventKind::Reject => write!(f, "reject"),
            ExecutionEventKind::PartialFill => write!(f, "partial_fill"),
            ExecutionEventKind::Fill => write!(f, "fill"),
            ExecutionEventKind::Terminal => write!(f, "terminal"),
        }
    }
}

impl std::str::FromStr for ExecutionEventKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "submit_intent" => Ok(Self::SubmitIntent),
            "exchange_ack" => Ok(Self::ExchangeAck),
            "cancel_request" => Ok(Self::CancelRequest),
            "cancel_ack" => Ok(Self::CancelAck),
            "reject" => Ok(Self::Reject),
            "partial_fill" => Ok(Self::PartialFill),
            "fill" => Ok(Self::Fill),
            "terminal" => Ok(Self::Terminal),
            other => Err(format!("unknown execution event kind: {other}")),
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEvent {
    pub event_timestamp_us: u64,
    pub asset_id: Option<AssetId>,
    pub order_id: String,
    pub client_order_id: Option<String>,
    pub venue_order_id: Option<String>,
    pub kind: ExecutionEventKind,
    pub side: Option<Side>,
    pub price: Option<FixedPrice>,
    pub size: Option<FixedSize>,
    pub status: Option<String>,
    pub reason: Option<String>,
    pub latency: LatencyTrace,
}

/// Persisted record routed through storage sinks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedRecord {
    Book(BookEvent),
    Trade(TradeEvent),
    Ingest(IngestEvent),
    Checkpoint(BookCheckpoint),
    Validation(ReplayValidation),
    Execution(ExecutionEvent),
}

impl PersistedRecord {
    pub fn dataset_name(&self) -> &'static str {
        match self {
            PersistedRecord::Book(_) => "book_events",
            PersistedRecord::Trade(_) => "trade_events",
            PersistedRecord::Ingest(_) => "ingest_events",
            PersistedRecord::Checkpoint(_) => "book_checkpoints",
            PersistedRecord::Validation(_) => "replay_validations",
            PersistedRecord::Execution(_) => "execution_events",
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
            PersistedRecord::Execution(event) => event
                .asset_id
                .as_ref()
                .map(|id| id.as_str())
                .unwrap_or("global"),
        }
    }

    pub fn partition_timestamp_us(&self) -> u64 {
        match self {
            PersistedRecord::Book(event) => event.provenance.recv_timestamp_us,
            PersistedRecord::Trade(event) => event.provenance.recv_timestamp_us,
            PersistedRecord::Ingest(event) => event.provenance.recv_timestamp_us,
            PersistedRecord::Checkpoint(event) => event.checkpoint_timestamp_us,
            PersistedRecord::Validation(event) => event.persisted_at_us,
            PersistedRecord::Execution(event) => event.event_timestamp_us,
        }
    }

    /// Mutable access to the record's `EventProvenance`, if it carries one.
    /// `ReplayValidation` and `ExecutionEvent` have no provenance and return
    /// `None`. Used at the single ingest serialization point to stamp the
    /// monotonic `ingest_ordinal` (audit A.116).
    pub fn provenance_mut(&mut self) -> Option<&mut EventProvenance> {
        match self {
            PersistedRecord::Book(event) => Some(&mut event.provenance),
            PersistedRecord::Trade(event) => Some(&mut event.provenance),
            PersistedRecord::Ingest(event) => Some(&mut event.provenance),
            PersistedRecord::Checkpoint(event) => Some(&mut event.provenance),
            PersistedRecord::Validation(_) => None,
            PersistedRecord::Execution(_) => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketDataWindow {
    pub book_events: Vec<BookEvent>,
    pub trade_events: Vec<TradeEvent>,
    pub ingest_events: Vec<IngestEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionWindow {
    pub execution_events: Vec<ExecutionEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_trace_monotonicity() {
        // In-order (with gaps) is monotonic.
        let ok = LatencyTrace::from_optional_timestamps(
            Some(100),
            None,
            Some(200),
            None,
            Some(300),
            Some(300),
        );
        assert!(ok.is_monotonic());

        // An earlier stage after a later one is a violation.
        let bad = LatencyTrace::from_optional_timestamps(
            Some(100),
            Some(50), // normalization_done before market_data_recv
            None,
            None,
            None,
            None,
        );
        assert!(!bad.is_monotonic());

        // All-absent is vacuously monotonic.
        assert!(LatencyTrace::default().is_monotonic());
    }

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
    fn test_book_event_serde() {
        let event = BookEvent {
            asset_id: AssetId::new("test-token"),
            kind: BookEventKind::Delta,
            side: Side::Bid,
            price: FixedPrice::from_f64(0.55).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: sample_provenance(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, event2);
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
    fn test_side_display() {
        assert_eq!(format!("{}", Side::Bid), "Bid");
        assert_eq!(format!("{}", Side::Ask), "Ask");
    }

    // --- Side tests ---

    #[test]
    fn side_serde_roundtrip() {
        for side in [Side::Bid, Side::Ask] {
            let json = serde_json::to_string(&side).unwrap();
            let s2: Side = serde_json::from_str(&json).unwrap();
            assert_eq!(side, s2);
        }
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

    // --- DataSource tests ---

    #[test]
    fn data_source_display_all_variants() {
        assert_eq!(format!("{}", DataSource::WebSocket), "websocket");
        assert_eq!(format!("{}", DataSource::RestSnapshot), "rest_snapshot");
        assert_eq!(
            format!("{}", DataSource::ReplayValidator),
            "replay_validator"
        );
        assert_eq!(format!("{}", DataSource::Strategy), "strategy");
        assert_eq!(format!("{}", DataSource::Exchange), "exchange");
        assert_eq!(format!("{}", DataSource::System), "system");
    }

    #[test]
    fn data_source_serde_roundtrip_all() {
        let variants = [
            DataSource::WebSocket,
            DataSource::RestSnapshot,
            DataSource::ReplayValidator,
            DataSource::Strategy,
            DataSource::Exchange,
            DataSource::System,
        ];
        for src in variants {
            let json = serde_json::to_string(&src).unwrap();
            let s2: DataSource = serde_json::from_str(&json).unwrap();
            assert_eq!(src, s2);
        }
    }

    // --- BookEventKind tests ---

    #[test]
    fn book_event_kind_display() {
        assert_eq!(format!("{}", BookEventKind::Snapshot), "Snapshot");
        assert_eq!(format!("{}", BookEventKind::Delta), "Delta");
    }

    #[test]
    fn book_event_kind_serde_roundtrip() {
        for kind in [BookEventKind::Snapshot, BookEventKind::Delta] {
            let json = serde_json::to_string(&kind).unwrap();
            let k2: BookEventKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, k2);
        }
    }

    // --- TradeFidelity tests ---

    #[test]
    fn trade_fidelity_display() {
        assert_eq!(format!("{}", TradeFidelity::Partial), "partial");
        assert_eq!(format!("{}", TradeFidelity::Full), "full");
    }

    #[test]
    fn trade_fidelity_serde_roundtrip() {
        for fid in [TradeFidelity::Partial, TradeFidelity::Full] {
            let json = serde_json::to_string(&fid).unwrap();
            let f2: TradeFidelity = serde_json::from_str(&json).unwrap();
            assert_eq!(fid, f2);
        }
    }

    // --- IngestEventKind tests ---

    #[test]
    fn ingest_event_kind_display_all() {
        assert_eq!(
            format!("{}", IngestEventKind::ReconnectStart),
            "reconnect_start"
        );
        assert_eq!(
            format!("{}", IngestEventKind::ReconnectSuccess),
            "reconnect_success"
        );
        assert_eq!(format!("{}", IngestEventKind::SequenceGap), "sequence_gap");
        assert_eq!(
            format!("{}", IngestEventKind::StaleSnapshotSkip),
            "stale_snapshot_skip"
        );
        assert_eq!(format!("{}", IngestEventKind::SourceReset), "source_reset");
    }

    #[test]
    fn ingest_event_kind_serde_roundtrip_all() {
        let kinds = [
            IngestEventKind::ReconnectStart,
            IngestEventKind::ReconnectSuccess,
            IngestEventKind::SequenceGap,
            IngestEventKind::StaleSnapshotSkip,
            IngestEventKind::SourceReset,
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let k2: IngestEventKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, k2);
        }
    }

    // --- ReplayMode tests ---

    #[test]
    fn replay_mode_display() {
        assert_eq!(format!("{}", ReplayMode::RecvTime), "recv_time");
        assert_eq!(format!("{}", ReplayMode::ExchangeTime), "exchange_time");
    }

    #[test]
    fn replay_mode_serde_roundtrip() {
        for mode in [ReplayMode::RecvTime, ReplayMode::ExchangeTime] {
            let json = serde_json::to_string(&mode).unwrap();
            let m2: ReplayMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, m2);
        }
    }

    // --- ExecutionEventKind tests ---

    #[test]
    fn execution_event_kind_display_all() {
        let pairs = [
            (ExecutionEventKind::SubmitIntent, "submit_intent"),
            (ExecutionEventKind::ExchangeAck, "exchange_ack"),
            (ExecutionEventKind::CancelRequest, "cancel_request"),
            (ExecutionEventKind::CancelAck, "cancel_ack"),
            (ExecutionEventKind::Reject, "reject"),
            (ExecutionEventKind::PartialFill, "partial_fill"),
            (ExecutionEventKind::Fill, "fill"),
            (ExecutionEventKind::Terminal, "terminal"),
        ];
        for (kind, expected) in pairs {
            assert_eq!(format!("{kind}"), expected);
        }
    }

    #[test]
    fn execution_event_kind_from_str_all() {
        let pairs = [
            ("submit_intent", ExecutionEventKind::SubmitIntent),
            ("exchange_ack", ExecutionEventKind::ExchangeAck),
            ("cancel_request", ExecutionEventKind::CancelRequest),
            ("cancel_ack", ExecutionEventKind::CancelAck),
            ("reject", ExecutionEventKind::Reject),
            ("partial_fill", ExecutionEventKind::PartialFill),
            ("fill", ExecutionEventKind::Fill),
            ("terminal", ExecutionEventKind::Terminal),
        ];
        for (input, expected) in pairs {
            assert_eq!(input.parse::<ExecutionEventKind>().unwrap(), expected);
        }
    }

    #[test]
    fn execution_event_kind_from_str_invalid() {
        assert!("unknown".parse::<ExecutionEventKind>().is_err());
        assert!("".parse::<ExecutionEventKind>().is_err());
    }

    #[test]
    fn execution_event_kind_serde_roundtrip_all() {
        let kinds = [
            ExecutionEventKind::SubmitIntent,
            ExecutionEventKind::ExchangeAck,
            ExecutionEventKind::CancelRequest,
            ExecutionEventKind::CancelAck,
            ExecutionEventKind::Reject,
            ExecutionEventKind::PartialFill,
            ExecutionEventKind::Fill,
            ExecutionEventKind::Terminal,
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let k2: ExecutionEventKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, k2);
        }
    }

    // --- TradeEvent serde round-trip ---

    #[test]
    fn trade_event_serde_roundtrip() {
        let event = TradeEvent {
            asset_id: AssetId::new("tok-1"),
            price: FixedPrice::from_f64(0.75).unwrap(),
            size: Some(FixedSize::from_f64(50.0).unwrap()),
            side: Some(Side::Ask),
            trade_id: Some("trade-abc".to_string()),
            fidelity: TradeFidelity::Full,
            provenance: sample_provenance(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: TradeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, event2);
    }

    #[test]
    fn trade_event_with_none_fields() {
        let event = TradeEvent {
            asset_id: AssetId::new("tok-2"),
            price: FixedPrice::ZERO,
            size: None,
            side: None,
            trade_id: None,
            fidelity: TradeFidelity::Partial,
            provenance: sample_provenance(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: TradeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, event2);
    }

    // --- IngestEvent serde round-trip ---

    #[test]
    fn ingest_event_serde_roundtrip() {
        let event = IngestEvent {
            asset_id: Some(AssetId::new("tok-3")),
            kind: IngestEventKind::SequenceGap,
            provenance: sample_provenance(),
            expected_sequence: Some(10),
            observed_sequence: Some(15),
            details: Some("missed 5 updates".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: IngestEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, event2);
    }

    #[test]
    fn ingest_event_global_serde() {
        let event = IngestEvent {
            asset_id: None,
            kind: IngestEventKind::ReconnectStart,
            provenance: sample_provenance(),
            expected_sequence: None,
            observed_sequence: None,
            details: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: IngestEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, event2);
    }

    // --- BookCheckpoint serde round-trip ---

    #[test]
    fn book_checkpoint_serde_roundtrip() {
        let checkpoint = BookCheckpoint {
            asset_id: AssetId::new("tok-4"),
            checkpoint_timestamp_us: 2_000_000,
            provenance: sample_provenance(),
            bids: vec![PriceLevel {
                price: FixedPrice::from_f64(0.50).unwrap(),
                size: FixedSize::from_f64(100.0).unwrap(),
            }],
            asks: vec![PriceLevel {
                price: FixedPrice::from_f64(0.55).unwrap(),
                size: FixedSize::from_f64(200.0).unwrap(),
            }],
            wal_offset: Some(12345),
        };
        let json = serde_json::to_string(&checkpoint).unwrap();
        let cp2: BookCheckpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(checkpoint, cp2);
    }

    #[test]
    fn book_checkpoint_empty_levels() {
        let checkpoint = BookCheckpoint {
            asset_id: AssetId::new("tok-5"),
            checkpoint_timestamp_us: 0,
            provenance: sample_provenance(),
            bids: vec![],
            asks: vec![],
            wal_offset: None,
        };
        let json = serde_json::to_string(&checkpoint).unwrap();
        let cp2: BookCheckpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(checkpoint, cp2);
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

    // --- ReplayValidation serde round-trip ---

    #[test]
    fn replay_validation_serde_roundtrip() {
        let validation = ReplayValidation {
            asset_id: AssetId::new("tok-7"),
            mode: ReplayMode::RecvTime,
            replay_timestamp_us: 1_000_000,
            reference_timestamp_us: 999_000,
            matched: true,
            mismatch_summary: None,
            persisted_at_us: 2_000_000,
        };
        let json = serde_json::to_string(&validation).unwrap();
        let v2: ReplayValidation = serde_json::from_str(&json).unwrap();
        assert_eq!(validation, v2);
    }

    #[test]
    fn replay_validation_with_mismatch() {
        let validation = ReplayValidation {
            asset_id: AssetId::new("tok-8"),
            mode: ReplayMode::ExchangeTime,
            replay_timestamp_us: 1_000_000,
            reference_timestamp_us: 999_000,
            matched: false,
            mismatch_summary: Some("3 bid levels differ".to_string()),
            persisted_at_us: 2_000_000,
        };
        let json = serde_json::to_string(&validation).unwrap();
        let v2: ReplayValidation = serde_json::from_str(&json).unwrap();
        assert_eq!(validation, v2);
    }

    // --- ExecutionEvent serde round-trip ---

    #[test]
    fn execution_event_serde_roundtrip() {
        let event = ExecutionEvent {
            event_timestamp_us: 5_000_000,
            asset_id: Some(AssetId::new("tok-9")),
            order_id: "order-1".to_string(),
            client_order_id: Some("client-1".to_string()),
            venue_order_id: Some("venue-1".to_string()),
            kind: ExecutionEventKind::Fill,
            side: Some(Side::Bid),
            price: Some(FixedPrice::from_f64(0.60).unwrap()),
            size: Some(FixedSize::from_f64(25.0).unwrap()),
            status: Some("filled".to_string()),
            reason: None,
            latency: LatencyTrace {
                market_data_recv_us: Some(1_000_000),
                normalization_done_us: Some(1_001_000),
                strategy_decision_us: Some(1_002_000),
                order_submit_us: Some(1_003_000),
                exchange_ack_us: Some(1_004_000),
                exchange_fill_us: Some(1_005_000),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: ExecutionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, event2);
    }

    #[test]
    fn execution_event_minimal() {
        let event = ExecutionEvent {
            event_timestamp_us: 0,
            asset_id: None,
            order_id: "order-min".to_string(),
            client_order_id: None,
            venue_order_id: None,
            kind: ExecutionEventKind::SubmitIntent,
            side: None,
            price: None,
            size: None,
            status: None,
            reason: None,
            latency: LatencyTrace::default(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: ExecutionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, event2);
    }

    // --- LatencyTrace tests ---

    #[test]
    fn latency_trace_default_all_none() {
        let lt = LatencyTrace::default();
        assert_eq!(lt.market_data_recv_us, None);
        assert_eq!(lt.normalization_done_us, None);
        assert_eq!(lt.strategy_decision_us, None);
        assert_eq!(lt.order_submit_us, None);
        assert_eq!(lt.exchange_ack_us, None);
        assert_eq!(lt.exchange_fill_us, None);
    }

    #[test]
    fn latency_trace_from_optional_timestamps() {
        let lt = LatencyTrace::from_optional_timestamps(
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            Some(5),
            Some(6),
        );
        assert_eq!(lt.market_data_recv_us, Some(1));
        assert_eq!(lt.exchange_fill_us, Some(6));
    }

    #[test]
    fn latency_trace_serde_roundtrip() {
        let lt = LatencyTrace {
            market_data_recv_us: Some(100),
            normalization_done_us: None,
            strategy_decision_us: Some(200),
            order_submit_us: None,
            exchange_ack_us: None,
            exchange_fill_us: Some(300),
        };
        let json = serde_json::to_string(&lt).unwrap();
        let lt2: LatencyTrace = serde_json::from_str(&json).unwrap();
        assert_eq!(lt, lt2);
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
            (
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: 0,
                    asset_id: None,
                    order_id: "o".to_string(),
                    client_order_id: None,
                    venue_order_id: None,
                    kind: ExecutionEventKind::SubmitIntent,
                    side: None,
                    price: None,
                    size: None,
                    status: None,
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
                "execution_events",
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
    fn persisted_record_asset_partition_global_execution() {
        let record = PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: 0,
            asset_id: None,
            order_id: "o".to_string(),
            client_order_id: None,
            venue_order_id: None,
            kind: ExecutionEventKind::Terminal,
            side: None,
            price: None,
            size: None,
            status: None,
            reason: None,
            latency: LatencyTrace::default(),
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

        // Execution uses event_timestamp_us
        let r = PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: 555,
            asset_id: None,
            order_id: "o".to_string(),
            client_order_id: None,
            venue_order_id: None,
            kind: ExecutionEventKind::SubmitIntent,
            side: None,
            price: None,
            size: None,
            status: None,
            reason: None,
            latency: LatencyTrace::default(),
        });
        assert_eq!(r.partition_timestamp_us(), 555);
    }

    #[test]
    fn persisted_record_serde_roundtrip_all_variants() {
        let prov = sample_provenance();
        let records = vec![
            PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("t"),
                kind: BookEventKind::Delta,
                side: Side::Ask,
                price: FixedPrice::from_f64(0.5).unwrap(),
                size: FixedSize::new(1_000_000),
                provenance: prov.clone(),
            }),
            PersistedRecord::Trade(TradeEvent {
                asset_id: AssetId::new("t"),
                price: FixedPrice::from_f64(0.75).unwrap(),
                size: Some(FixedSize::new(500_000)),
                side: Some(Side::Bid),
                trade_id: Some("tid".to_string()),
                fidelity: TradeFidelity::Full,
                provenance: prov.clone(),
            }),
            PersistedRecord::Ingest(IngestEvent {
                asset_id: Some(AssetId::new("t")),
                kind: IngestEventKind::SequenceGap,
                provenance: prov.clone(),
                expected_sequence: Some(10),
                observed_sequence: Some(12),
                details: Some("gap".to_string()),
            }),
            PersistedRecord::Checkpoint(BookCheckpoint {
                asset_id: AssetId::new("t"),
                checkpoint_timestamp_us: 99,
                provenance: prov.clone(),
                bids: vec![PriceLevel {
                    price: FixedPrice::new(5000).unwrap(),
                    size: FixedSize::new(1_000_000),
                }],
                asks: vec![],
                wal_offset: Some(42),
            }),
            PersistedRecord::Validation(ReplayValidation {
                asset_id: AssetId::new("t"),
                mode: ReplayMode::ExchangeTime,
                replay_timestamp_us: 100,
                reference_timestamp_us: 99,
                matched: false,
                mismatch_summary: Some("diff".to_string()),
                persisted_at_us: 200,
            }),
            PersistedRecord::Execution(ExecutionEvent {
                event_timestamp_us: 300,
                asset_id: Some(AssetId::new("t")),
                order_id: "o1".to_string(),
                client_order_id: None,
                venue_order_id: None,
                kind: ExecutionEventKind::Reject,
                side: None,
                price: None,
                size: None,
                status: None,
                reason: Some("insufficient funds".to_string()),
                latency: LatencyTrace::default(),
            }),
        ];
        for record in &records {
            let json = serde_json::to_string(record).unwrap();
            let r2: PersistedRecord = serde_json::from_str(&json).unwrap();
            assert_eq!(*record, r2);
        }
    }

    // --- MarketDataWindow / ExecutionWindow tests ---

    #[test]
    fn market_data_window_default() {
        let w = MarketDataWindow::default();
        assert!(w.book_events.is_empty());
        assert!(w.trade_events.is_empty());
        assert!(w.ingest_events.is_empty());
    }

    #[test]
    fn execution_window_default() {
        let w = ExecutionWindow::default();
        assert!(w.execution_events.is_empty());
    }

    #[test]
    fn market_data_window_serde_roundtrip() {
        let prov = sample_provenance();
        let w = MarketDataWindow {
            book_events: vec![BookEvent {
                asset_id: AssetId::new("t"),
                kind: BookEventKind::Delta,
                side: Side::Bid,
                price: FixedPrice::from_f64(0.5).unwrap(),
                size: FixedSize::new(1_000_000),
                provenance: prov.clone(),
            }],
            trade_events: vec![],
            ingest_events: vec![],
        };
        let json = serde_json::to_string(&w).unwrap();
        let w2: MarketDataWindow = serde_json::from_str(&json).unwrap();
        assert_eq!(w, w2);
    }

    // --- EventProvenance edge values ---

    #[test]
    fn event_provenance_all_none_optional_fields() {
        let prov = EventProvenance {
            recv_timestamp_us: 0,
            exchange_timestamp_us: 0,
            source: DataSource::System,
            source_event_id: None,
            source_session_id: None,
            sequence: None,
            ingest_ordinal: None,
        };
        let json = serde_json::to_string(&prov).unwrap();
        let prov2: EventProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(prov, prov2);
    }

    #[test]
    fn event_provenance_max_timestamps() {
        let prov = EventProvenance {
            recv_timestamp_us: u64::MAX,
            exchange_timestamp_us: u64::MAX,
            source: DataSource::Exchange,
            source_event_id: Some("max".to_string()),
            source_session_id: Some("max-session".to_string()),
            sequence: Some(Sequence::new(u64::MAX)),
            ingest_ordinal: None,
        };
        let json = serde_json::to_string(&prov).unwrap();
        let prov2: EventProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(prov, prov2);
    }

    // --- PriceLevel tests ---

    #[test]
    fn price_level_serde_roundtrip() {
        let pl = PriceLevel {
            price: FixedPrice::from_f64(0.9999).unwrap(),
            size: FixedSize::new(999_999_999),
        };
        let json = serde_json::to_string(&pl).unwrap();
        let pl2: PriceLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(pl, pl2);
    }
}
