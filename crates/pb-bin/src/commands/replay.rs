use anyhow::{bail, Result};
use config::Config;
use pb_types::AssetId;

pub async fn run(settings: Config, token: String, at_us: u64, source: String) -> Result<()> {
    tracing::info!(token = %token, at_us, source = %source, "replaying orderbook state");

    let asset_id = AssetId::new(token);

    match source.as_str() {
        "parquet" => {
            let base_path = settings
                .get_string("storage.parquet_base_path")
                .unwrap_or_else(|_| "./data".to_string());
            let reader = pb_replay::ParquetReader::new(&base_path);
            let engine = pb_replay::ReplayEngine::new(reader);
            let book = engine.reconstruct_at(&asset_id, at_us).await?;
            print_book(&book);
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
            let book = engine.reconstruct_at(&asset_id, at_us).await?;
            print_book(&book);
        }
        other => bail!("unknown source: {other}. Use 'parquet' or 'clickhouse'"),
    }

    Ok(())
}

fn print_book(book: &pb_book::L2Book) {
    println!("Reconstructed L2Book for {}", book.asset_id);
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

    println!("\nBids:");
    for (price, size) in book.bids_sorted() {
        println!("  {} | {}", price, size);
    }
    println!("\nAsks:");
    for (price, size) in book.asks_sorted() {
        println!("  {} | {}", price, size);
    }
}
