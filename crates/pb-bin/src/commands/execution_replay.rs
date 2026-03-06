use anyhow::{bail, Result};
use config::Config;

pub async fn run(
    settings: Config,
    order_id: Option<String>,
    start_us: u64,
    end_us: u64,
    source: String,
) -> Result<()> {
    tracing::info!(
        ?order_id,
        start_us,
        end_us,
        source = %source,
        "replaying execution history"
    );

    let events = match source.as_str() {
        "parquet" => {
            let base_path = settings
                .get_string("storage.parquet_base_path")
                .unwrap_or_else(|_| "./data".to_string());
            let reader = pb_replay::ParquetReader::new(&base_path);
            let engine = pb_replay::ReplayEngine::new(reader);
            engine
                .execution_events(order_id.as_deref(), start_us, end_us)
                .await?
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
            engine
                .execution_events(order_id.as_deref(), start_us, end_us)
                .await?
        }
        other => bail!("unknown source: {other}. Use 'parquet' or 'clickhouse'"),
    };

    println!("Execution events: {}", events.len());
    for event in events {
        println!(
            "{} order_id={} kind={} asset={} status={:?} price={:?} size={:?}",
            event.event_timestamp_us,
            event.order_id,
            event.kind,
            event
                .asset_id
                .as_ref()
                .map(|id| id.as_str())
                .unwrap_or("n/a"),
            event.status,
            event.price,
            event.size
        );
    }

    Ok(())
}
