use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

mod commands;

#[derive(Parser)]
#[command(
    name = "poly-book",
    version,
    about = "Polymarket BTC 5-Min Orderbook System"
)]
struct Cli {
    /// Config file path
    #[arg(long, default_value = "config/default.toml")]
    config: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Discover active BTC 5-minute prediction markets
    Discover {
        /// Filter by keyword in market title
        #[arg(long)]
        filter: Option<String>,
    },
    /// Start live orderbook ingestion
    Ingest {
        /// Comma-separated token IDs to subscribe to
        #[arg(long)]
        tokens: Option<String>,
        /// Enable Parquet storage
        #[arg(long, default_value_t = true)]
        parquet: bool,
        /// Enable ClickHouse storage
        #[arg(long, default_value_t = false)]
        clickhouse: bool,
        /// Enable metrics server
        #[arg(long, default_value_t = true)]
        metrics: bool,
    },
    /// Replay historical orderbook state at a specific timestamp
    Replay {
        /// Token ID to replay
        #[arg(long)]
        token: String,
        /// Target timestamp in microseconds since epoch
        #[arg(long)]
        at: u64,
        /// Data source: "parquet" or "clickhouse"
        #[arg(long, default_value = "parquet")]
        source: String,
    },
    /// Backfill historical data via REST API snapshots
    Backfill {
        /// Comma-separated token IDs to backfill
        #[arg(long)]
        tokens: String,
        /// Interval between snapshot fetches in seconds
        #[arg(long, default_value_t = 60)]
        interval_secs: u64,
        /// Duration to run backfill in minutes (0 = indefinite)
        #[arg(long, default_value_t = 0)]
        duration_mins: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log_level));
    fmt().with_env_filter(filter).init();

    // Load config
    let settings = config::Config::builder()
        .add_source(config::File::with_name(&cli.config).required(false))
        .add_source(config::Environment::with_prefix("PB").separator("__"))
        .build()?;

    match cli.command {
        Commands::Discover { filter } => {
            commands::discover::run(settings, filter).await?;
        }
        Commands::Ingest {
            tokens,
            parquet,
            clickhouse,
            metrics,
        } => {
            commands::ingest::run(settings, tokens, parquet, clickhouse, metrics).await?;
        }
        Commands::Replay { token, at, source } => {
            commands::replay::run(settings, token, at, source).await?;
        }
        Commands::Backfill {
            tokens,
            interval_secs,
            duration_mins,
        } => {
            commands::backfill::run(settings, tokens, interval_secs, duration_mins).await?;
        }
    }

    Ok(())
}
