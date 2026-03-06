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
}

impl std::fmt::Display for IngestEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestEventKind::ReconnectStart => write!(f, "reconnect_start"),
            IngestEventKind::ReconnectSuccess => write!(f, "reconnect_success"),
            IngestEventKind::SequenceGap => write!(f, "sequence_gap"),
            IngestEventKind::StaleSnapshotSkip => write!(f, "stale_snapshot_skip"),
            IngestEventKind::SourceReset => write!(f, "source_reset"),
        }
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

    fn sample_provenance() -> EventProvenance {
        EventProvenance {
            recv_timestamp_us: 1_000_000,
            exchange_timestamp_us: 999_000,
            source: DataSource::WebSocket,
            source_event_id: Some("abc".to_string()),
            source_session_id: Some("session-1".to_string()),
            sequence: Some(Sequence::new(42)),
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
}
