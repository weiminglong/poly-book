use std::str::FromStr;

use anyhow::{bail, Context, Result};
use clap::Args;
use config::Config;
use serde::Deserialize;

use pb_store::{ClickHouseRecordWriter, ParquetRecordWriter};
use pb_types::{
    AssetId, ExecutionEvent, ExecutionEventKind, FixedPrice, FixedSize, LatencyTrace,
    PersistedRecord, Side,
};

#[derive(Args)]
pub struct ExecutionAppendArgs {
    /// Data sink: "parquet" or "clickhouse"
    #[arg(long, default_value = "parquet")]
    pub source: String,
    /// Skip within-batch order-lifecycle validation (for backfilling partial
    /// histories where, e.g., a SubmitIntent is intentionally absent).
    #[arg(long, default_value_t = false)]
    pub skip_lifecycle_checks: bool,
    /// Optional path to a JSON object or array payload
    #[arg(long)]
    pub json: Option<String>,
    /// Required unless --json is used
    #[arg(long)]
    pub order_id: Option<String>,
    /// Required unless --json is used
    #[arg(long)]
    pub event_kind: Option<String>,
    /// Required unless --json is used
    #[arg(long)]
    pub event_timestamp_us: Option<u64>,
    #[arg(long)]
    pub asset_id: Option<String>,
    #[arg(long)]
    pub client_order_id: Option<String>,
    #[arg(long)]
    pub venue_order_id: Option<String>,
    #[arg(long)]
    pub side: Option<String>,
    #[arg(long)]
    pub price: Option<String>,
    #[arg(long)]
    pub size: Option<String>,
    #[arg(long)]
    pub status: Option<String>,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long)]
    pub market_data_recv_us: Option<u64>,
    #[arg(long)]
    pub normalization_done_us: Option<u64>,
    #[arg(long)]
    pub strategy_decision_us: Option<u64>,
    #[arg(long)]
    pub order_submit_us: Option<u64>,
    #[arg(long)]
    pub exchange_ack_us: Option<u64>,
    #[arg(long)]
    pub exchange_fill_us: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionAppendInput {
    order_id: String,
    event_kind: String,
    event_timestamp_us: u64,
    asset_id: Option<String>,
    client_order_id: Option<String>,
    venue_order_id: Option<String>,
    side: Option<String>,
    price: Option<String>,
    size: Option<String>,
    status: Option<String>,
    reason: Option<String>,
    market_data_recv_us: Option<u64>,
    normalization_done_us: Option<u64>,
    strategy_decision_us: Option<u64>,
    order_submit_us: Option<u64>,
    exchange_ack_us: Option<u64>,
    exchange_fill_us: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ExecutionAppendPayload {
    One(Box<ExecutionAppendInput>),
    Many(Vec<ExecutionAppendInput>),
}

impl ExecutionAppendPayload {
    fn into_records(self) -> Result<Vec<PersistedRecord>> {
        match self {
            Self::One(event) => Ok(vec![PersistedRecord::Execution((*event).try_into()?)]),
            Self::Many(events) => events
                .into_iter()
                .map(|event| Ok(PersistedRecord::Execution(event.try_into()?)))
                .collect(),
        }
    }
}

/// Plausible microsecond-timestamp range, roughly [2001, 2100). A millisecond
/// (~1.7e12) or second (~1.7e9) value falls below the minimum and would be filed
/// into a 1970-era partition, making the event invisible to every time-windowed
/// query (audit finding A.61).
const MIN_EVENT_TS_US: u64 = 1_000_000_000_000_000;
const MAX_EVENT_TS_US: u64 = 4_102_444_800_000_000;

impl TryFrom<ExecutionAppendInput> for ExecutionEvent {
    type Error = anyhow::Error;

    fn try_from(value: ExecutionAppendInput) -> Result<Self, Self::Error> {
        if !(MIN_EVENT_TS_US..MAX_EVENT_TS_US).contains(&value.event_timestamp_us) {
            bail!(
                "event_timestamp_us {} is not a plausible microsecond timestamp \
                 (expected {MIN_EVENT_TS_US}..{MAX_EVENT_TS_US}); a millisecond or \
                 second value would be filed into a 1970 partition and become invisible",
                value.event_timestamp_us
            );
        }
        if value.order_id.trim().is_empty() {
            bail!("order_id must not be empty");
        }
        let kind = ExecutionEventKind::from_str(&value.event_kind)
            .map_err(|error| anyhow::anyhow!(error))?;
        let side = value
            .side
            .as_deref()
            .map(Side::from_str)
            .transpose()
            .map_err(|error| anyhow::anyhow!(error))?;
        let price = value
            .price
            .as_deref()
            .map(FixedPrice::try_from)
            .transpose()?;
        let size = value.size.as_deref().map(FixedSize::try_from).transpose()?;

        // A (partial) fill that carries no price or size is malformed — it would
        // record a trade with unknown economics (part of A.144).
        if matches!(
            kind,
            ExecutionEventKind::Fill | ExecutionEventKind::PartialFill
        ) && (price.is_none() || size.is_none())
        {
            bail!("{kind} events require both price and size");
        }

        let latency = LatencyTrace::from_optional_timestamps(
            value.market_data_recv_us,
            value.normalization_done_us,
            value.strategy_decision_us,
            value.order_submit_us,
            value.exchange_ack_us,
            value.exchange_fill_us,
        );
        // Reject a causally-inverted latency trace, which would otherwise render
        // as negative stage durations downstream (A.62).
        if !latency.is_monotonic() {
            bail!("latency stage timestamps are not in non-decreasing causal order");
        }

        Ok(ExecutionEvent {
            event_timestamp_us: value.event_timestamp_us,
            asset_id: value.asset_id.map(AssetId::new),
            order_id: value.order_id,
            client_order_id: value.client_order_id,
            venue_order_id: value.venue_order_id,
            kind,
            side,
            price,
            size,
            status: value.status,
            reason: value.reason,
            latency,
        })
    }
}

/// Returns true for execution event kinds that terminate an order's lifecycle.
/// After any of these, no further event is valid for that order.
fn is_terminal_kind(kind: ExecutionEventKind) -> bool {
    matches!(
        kind,
        ExecutionEventKind::Fill
            | ExecutionEventKind::CancelAck
            | ExecutionEventKind::Reject
            | ExecutionEventKind::Terminal
    )
}

/// Validate the order-lifecycle coherence of a batch of execution events
/// (audit finding A.144). Events are grouped per `order_id` and checked in
/// timestamp order for illegal transitions:
///
/// - an event after a terminal event (fill/ack/cancel after Fill/CancelAck/
///   Reject/Terminal),
/// - a `SubmitIntent` that arrives after other activity (ack/fill before submit),
/// - a second `SubmitIntent` for the same order,
/// - cumulative fill size exceeding the submitted size (when the submit carries
///   a size).
///
/// This is a *within-batch* check: it does not read existing history, so it is
/// safe for appends but cannot catch violations that span separate invocations.
/// Operators backfilling deliberately-partial histories can bypass it with
/// `--skip-lifecycle-checks`.
fn validate_execution_lifecycle(records: &[PersistedRecord]) -> Result<()> {
    use std::collections::BTreeMap;

    let mut by_order: BTreeMap<&str, Vec<&ExecutionEvent>> = BTreeMap::new();
    for record in records {
        if let PersistedRecord::Execution(event) = record {
            by_order.entry(&event.order_id).or_default().push(event);
        }
    }

    for (order_id, mut events) in by_order {
        // Stable sort by timestamp so same-timestamp events keep batch order.
        events.sort_by_key(|e| e.event_timestamp_us);

        let mut terminal: Option<ExecutionEventKind> = None;
        let mut saw_non_submit = false;
        let mut submitted = false;
        let mut submitted_size: Option<u64> = None;
        let mut cumulative_fill: u64 = 0;

        for event in events {
            if let Some(term) = terminal {
                bail!(
                    "order {order_id}: {} event at {} follows terminal {} event",
                    event.kind,
                    event.event_timestamp_us,
                    term
                );
            }
            match event.kind {
                ExecutionEventKind::SubmitIntent => {
                    if submitted {
                        bail!("order {order_id}: duplicate SubmitIntent");
                    }
                    if saw_non_submit {
                        bail!(
                            "order {order_id}: SubmitIntent at {} follows earlier activity \
                             (ack/fill before submit)",
                            event.event_timestamp_us
                        );
                    }
                    submitted = true;
                    submitted_size = event.size.map(|s| s.raw());
                }
                ExecutionEventKind::PartialFill | ExecutionEventKind::Fill => {
                    saw_non_submit = true;
                    cumulative_fill =
                        cumulative_fill.saturating_add(event.size.map(|s| s.raw()).unwrap_or(0));
                    if let Some(max) = submitted_size {
                        if cumulative_fill > max {
                            bail!(
                                "order {order_id}: cumulative fill size {cumulative_fill} exceeds \
                                 submitted size {max}"
                            );
                        }
                    }
                    if is_terminal_kind(event.kind) {
                        terminal = Some(event.kind);
                    }
                }
                other => {
                    saw_non_submit = true;
                    if is_terminal_kind(other) {
                        terminal = Some(other);
                    }
                }
            }
        }
    }
    Ok(())
}

pub async fn run(settings: Config, args: ExecutionAppendArgs) -> Result<()> {
    let ExecutionAppendArgs {
        source,
        skip_lifecycle_checks,
        json,
        order_id,
        event_kind,
        event_timestamp_us,
        asset_id,
        client_order_id,
        venue_order_id,
        side,
        price,
        size,
        status,
        reason,
        market_data_recv_us,
        normalization_done_us,
        strategy_decision_us,
        order_submit_us,
        exchange_ack_us,
        exchange_fill_us,
    } = args;

    let records = if let Some(path) = json {
        let payload = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read execution append JSON from {path}"))?;
        let parsed: ExecutionAppendPayload = serde_json::from_str(&payload)
            .with_context(|| format!("failed to parse execution append JSON from {path}"))?;
        parsed.into_records()?
    } else {
        let payload = ExecutionAppendInput {
            order_id: order_id.ok_or_else(|| anyhow::anyhow!("--order-id is required"))?,
            event_kind: event_kind.ok_or_else(|| anyhow::anyhow!("--event-kind is required"))?,
            event_timestamp_us: event_timestamp_us
                .ok_or_else(|| anyhow::anyhow!("--event-timestamp-us is required"))?,
            asset_id,
            client_order_id,
            venue_order_id,
            side,
            price,
            size,
            status,
            reason,
            market_data_recv_us,
            normalization_done_us,
            strategy_decision_us,
            order_submit_us,
            exchange_ack_us,
            exchange_fill_us,
        };
        vec![PersistedRecord::Execution(payload.try_into()?)]
    };

    // Reject incoherent order lifecycles within the batch unless explicitly
    // bypassed for partial-history backfills (A.144).
    if skip_lifecycle_checks {
        tracing::warn!("skipping order-lifecycle validation (--skip-lifecycle-checks)");
    } else {
        validate_execution_lifecycle(&records)?;
    }

    tracing::info!(source = %source, records = records.len(), "appending execution events");

    match source.as_str() {
        "parquet" => {
            let configured = resolve_parquet_base_path(&settings)?;
            let (store, base_path) = super::pipeline::build_object_store(&configured)?;
            let writer = ParquetRecordWriter::new(store, base_path);
            writer.write_batch(&records).await?;
        }
        "clickhouse" => {
            let writer = clickhouse_writer(&settings);
            writer.ensure_tables().await?;
            writer.write_batch(&records).await?;
        }
        other => bail!("unknown source: {other}. Use 'parquet' or 'clickhouse'"),
    }

    println!("Appended {} execution event(s)", records.len());
    Ok(())
}

fn resolve_parquet_base_path(settings: &Config) -> Result<String> {
    let base_path = settings
        .get_string("storage.parquet_base_path")
        .unwrap_or_else(|_| "./data".to_string());
    let base_path = std::path::Path::new(&base_path)
        .canonicalize()
        .or_else(|_| {
            std::fs::create_dir_all(&base_path)?;
            std::path::Path::new(&base_path).canonicalize()
        })?
        .to_string_lossy()
        .to_string();
    Ok(base_path)
}

fn clickhouse_writer(settings: &Config) -> ClickHouseRecordWriter {
    let ch_url = settings
        .get_string("storage.clickhouse_url")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());
    let ch_db = settings
        .get_string("storage.clickhouse_database")
        .unwrap_or_else(|_| "poly_book".to_string());
    let client = clickhouse::Client::default()
        .with_url(&ch_url)
        .with_database(&ch_db);
    ClickHouseRecordWriter::new(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_with_ts(ts: u64) -> ExecutionAppendInput {
        ExecutionAppendInput {
            order_id: "o1".to_string(),
            event_kind: "submit_intent".to_string(),
            event_timestamp_us: ts,
            asset_id: None,
            client_order_id: None,
            venue_order_id: None,
            side: None,
            price: None,
            size: None,
            status: None,
            reason: None,
            market_data_recv_us: None,
            normalization_done_us: None,
            strategy_decision_us: None,
            order_submit_us: None,
            exchange_ack_us: None,
            exchange_fill_us: None,
        }
    }

    #[test]
    fn accepts_microsecond_timestamp() {
        let ev = ExecutionEvent::try_from(input_with_ts(1_750_000_000_000_000));
        assert!(ev.is_ok());
    }

    #[test]
    fn rejects_empty_order_id() {
        let mut input = input_with_ts(1_750_000_000_000_000);
        input.order_id = "  ".to_string();
        assert!(ExecutionEvent::try_from(input).is_err());
    }

    #[test]
    fn rejects_fill_without_price_or_size() {
        let mut input = input_with_ts(1_750_000_000_000_000);
        input.event_kind = "fill".to_string();
        input.price = None;
        input.size = None;
        assert!(ExecutionEvent::try_from(input).is_err());

        // A fill with both price and size is accepted.
        let mut ok = input_with_ts(1_750_000_000_000_000);
        ok.event_kind = "fill".to_string();
        ok.price = Some("0.5".to_string());
        ok.size = Some("10".to_string());
        assert!(ExecutionEvent::try_from(ok).is_ok());
    }

    #[test]
    fn rejects_non_monotonic_latency_trace() {
        let mut input = input_with_ts(1_750_000_000_000_000);
        // normalization_done before market_data_recv -> causally inverted.
        input.market_data_recv_us = Some(100);
        input.normalization_done_us = Some(50);
        assert!(ExecutionEvent::try_from(input).is_err());
    }

    #[test]
    fn rejects_millisecond_and_second_timestamps() {
        // Millisecond (~1.75e12) and second (~1.75e9) values would land in a
        // 1970 partition; both must be rejected (A.61).
        assert!(ExecutionEvent::try_from(input_with_ts(1_750_000_000_000)).is_err());
        assert!(ExecutionEvent::try_from(input_with_ts(1_750_000_000)).is_err());
        assert!(ExecutionEvent::try_from(input_with_ts(0)).is_err());
    }

    // --- Order-lifecycle validation (A.144) ---

    fn exec(
        order_id: &str,
        kind: ExecutionEventKind,
        ts: u64,
        size: Option<u64>,
    ) -> PersistedRecord {
        PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: ts,
            asset_id: Some(AssetId::new("tok1")),
            order_id: order_id.to_string(),
            client_order_id: None,
            venue_order_id: None,
            kind,
            side: Some(Side::Bid),
            price: Some(FixedPrice::new(5000).unwrap()),
            size: size.map(FixedSize::new),
            status: None,
            reason: None,
            latency: LatencyTrace::default(),
        })
    }

    #[test]
    fn lifecycle_accepts_valid_order() {
        let batch = vec![
            exec("o1", ExecutionEventKind::SubmitIntent, 1000, Some(100)),
            exec("o1", ExecutionEventKind::ExchangeAck, 1001, None),
            exec("o1", ExecutionEventKind::PartialFill, 1002, Some(40)),
            exec("o1", ExecutionEventKind::Fill, 1003, Some(60)),
        ];
        assert!(validate_execution_lifecycle(&batch).is_ok());
    }

    #[test]
    fn lifecycle_rejects_event_after_terminal() {
        // Fill (terminal) then a later PartialFill.
        let batch = vec![
            exec("o1", ExecutionEventKind::SubmitIntent, 1000, Some(100)),
            exec("o1", ExecutionEventKind::Fill, 1002, Some(100)),
            exec("o1", ExecutionEventKind::PartialFill, 1003, Some(10)),
        ];
        let err = validate_execution_lifecycle(&batch).unwrap_err();
        assert!(err.to_string().contains("terminal"), "got {err}");
    }

    #[test]
    fn lifecycle_rejects_fill_after_cancel() {
        let batch = vec![
            exec("o1", ExecutionEventKind::SubmitIntent, 1000, Some(100)),
            exec("o1", ExecutionEventKind::CancelAck, 1002, None),
            exec("o1", ExecutionEventKind::Fill, 1003, Some(50)),
        ];
        assert!(validate_execution_lifecycle(&batch).is_err());
    }

    #[test]
    fn lifecycle_rejects_ack_before_submit() {
        // ExchangeAck at t=1000, SubmitIntent at t=1001 → submit after activity.
        let batch = vec![
            exec("o1", ExecutionEventKind::ExchangeAck, 1000, None),
            exec("o1", ExecutionEventKind::SubmitIntent, 1001, Some(100)),
        ];
        let err = validate_execution_lifecycle(&batch).unwrap_err();
        assert!(err.to_string().contains("before submit"), "got {err}");
    }

    #[test]
    fn lifecycle_rejects_overfill() {
        let batch = vec![
            exec("o1", ExecutionEventKind::SubmitIntent, 1000, Some(100)),
            exec("o1", ExecutionEventKind::PartialFill, 1001, Some(70)),
            exec("o1", ExecutionEventKind::PartialFill, 1002, Some(50)),
        ];
        let err = validate_execution_lifecycle(&batch).unwrap_err();
        assert!(err.to_string().contains("exceeds submitted"), "got {err}");
    }

    #[test]
    fn lifecycle_rejects_duplicate_submit() {
        let batch = vec![
            exec("o1", ExecutionEventKind::SubmitIntent, 1000, Some(100)),
            exec("o1", ExecutionEventKind::SubmitIntent, 1001, Some(100)),
        ];
        assert!(validate_execution_lifecycle(&batch).is_err());
    }

    #[test]
    fn lifecycle_allows_partial_history_without_submit() {
        // Backfill that begins mid-lifecycle (no SubmitIntent) carries no
        // submitted size, so cumulative-fill checks do not apply and the batch is
        // accepted (use --skip-lifecycle-checks only for harder cases).
        let batch = vec![
            exec("o1", ExecutionEventKind::PartialFill, 1002, Some(40)),
            exec("o1", ExecutionEventKind::Fill, 1003, Some(60)),
        ];
        assert!(validate_execution_lifecycle(&batch).is_ok());
    }

    #[test]
    fn lifecycle_isolates_distinct_orders() {
        // Two independent orders interleaved; each is individually valid.
        let batch = vec![
            exec("o1", ExecutionEventKind::SubmitIntent, 1000, Some(100)),
            exec("o2", ExecutionEventKind::SubmitIntent, 1000, Some(50)),
            exec("o1", ExecutionEventKind::Fill, 1002, Some(100)),
            exec("o2", ExecutionEventKind::Fill, 1002, Some(50)),
        ];
        assert!(validate_execution_lifecycle(&batch).is_ok());
    }
}
