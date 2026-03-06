use anyhow::{bail, Result};
use config::Config;
use pb_store::{ClickHouseRecordWriter, ParquetRecordWriter};
use pb_types::{AssetId, PersistedRecord, ReplayMode};
use std::sync::Arc;

pub async fn run(
    settings: Config,
    token: String,
    at_us: u64,
    source: String,
    mode: String,
    validate: bool,
) -> Result<()> {
    let replay_mode = parse_mode(&mode)?;
    tracing::info!(
        token = %token,
        at_us,
        source = %source,
        mode = %replay_mode,
        validate,
        "replaying orderbook state"
    );

    let asset_id = AssetId::new(token);

    match source.as_str() {
        "parquet" => {
            let base_path = settings
                .get_string("storage.parquet_base_path")
                .unwrap_or_else(|_| "./data".to_string());
            let reader = pb_replay::ParquetReader::new(&base_path);
            let engine = pb_replay::ReplayEngine::new(reader);
            let replay = engine.reconstruct_at(&asset_id, at_us, replay_mode).await?;
            let validation = if validate {
                engine
                    .replay_validation(&asset_id, at_us, replay_mode)
                    .await?
            } else {
                None
            };
            if let Some(validation) = validation.as_ref() {
                persist_validation_parquet(&settings, validation.clone()).await?;
            }
            print_book(&replay, validation.as_ref());
        }
        "clickhouse" => {
            let ch_url = settings
                .get_string("storage.clickhouse_url")
                .unwrap_or_else(|_| "http://localhost:8123".to_string());
            let ch_db = settings
                .get_string("storage.clickhouse_database")
                .unwrap_or_else(|_| "poly_book".to_string());
            let reader = pb_replay::ClickHouseReader::new(&ch_url, &ch_db);
            let engine = pb_replay::ReplayEngine::new(reader);
            let replay = engine.reconstruct_at(&asset_id, at_us, replay_mode).await?;
            let validation = if validate {
                engine
                    .replay_validation(&asset_id, at_us, replay_mode)
                    .await?
            } else {
                None
            };
            if let Some(validation) = validation.as_ref() {
                persist_validation_clickhouse(&settings, validation.clone()).await?;
            }
            print_book(&replay, validation.as_ref());
        }
        other => bail!("unknown source: {other}. Use 'parquet' or 'clickhouse'"),
    }

    Ok(())
}

fn parse_mode(mode: &str) -> Result<ReplayMode> {
    match mode {
        "recv_time" => Ok(ReplayMode::RecvTime),
        "exchange_time" => Ok(ReplayMode::ExchangeTime),
        other => bail!("unknown replay mode: {other}. Use 'recv_time' or 'exchange_time'"),
    }
}

async fn persist_validation_parquet(
    settings: &Config,
    validation: pb_types::ReplayValidation,
) -> Result<()> {
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
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let writer = ParquetRecordWriter::new(store, base_path);
    writer
        .write_record(PersistedRecord::Validation(validation))
        .await?;
    Ok(())
}

async fn persist_validation_clickhouse(
    settings: &Config,
    validation: pb_types::ReplayValidation,
) -> Result<()> {
    let ch_url = settings
        .get_string("storage.clickhouse_url")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());
    let ch_db = settings
        .get_string("storage.clickhouse_database")
        .unwrap_or_else(|_| "poly_book".to_string());
    let writer = ClickHouseRecordWriter::new(
        clickhouse::Client::default()
            .with_url(&ch_url)
            .with_database(&ch_db),
    );
    writer.ensure_tables().await?;
    writer
        .write_record(PersistedRecord::Validation(validation))
        .await?;
    Ok(())
}

fn print_book(replay: &pb_replay::ReplayResult, validation: Option<&pb_types::ReplayValidation>) {
    let book = &replay.book;
    println!("Reconstructed L2Book for {}", book.asset_id);
    println!("  Mode: {}", replay.mode);
    println!("  Used checkpoint: {}", replay.used_checkpoint);
    println!("  Continuity events: {}", replay.continuity_events.len());
    println!("  Sequence: {}", book.sequence);
    println!("  Bid levels: {}", book.bid_depth());
    println!("  Ask levels: {}", book.ask_depth());

    if let Some((price, size)) = book.best_bid() {
        println!("  Best bid: {} @ {}", price, size);
    }
    if let Some((price, size)) = book.best_ask() {
        println!("  Best ask: {} @ {}", price, size);
    }
    if let Some(mid) = book.mid_price() {
        println!("  Mid price: {mid:.4}");
    }
    if let Some(spread) = book.spread() {
        println!("  Spread: {spread:.4}");
    }
    if let Some(validation) = validation {
        println!("  Validation matched: {}", validation.matched);
        if let Some(summary) = &validation.mismatch_summary {
            println!("  Validation summary: {summary}");
        }
    }

    if !replay.continuity_events.is_empty() {
        println!("\nContinuity:");
        for event in &replay.continuity_events {
            let asset = event
                .asset_id
                .as_ref()
                .map(|value| value.as_str())
                .unwrap_or("global");
            println!(
                "  {} @ {} [{}]",
                event.kind, event.provenance.recv_timestamp_us, asset
            );
        }
    }

    println!("\nBids:");
    for (price, size) in book.bids_sorted() {
        println!("  {} | {}", price, size);
    }
    println!("\nAsks:");
    for (price, size) in book.asks_sorted() {
        println!("  {} | {}", price, size);
    }
}
