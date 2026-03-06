use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
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
        /// Maximum number of events to scan (paginated in batches of 100)
        #[arg(long, default_value_t = 500)]
        limit: u64,
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

    // Load config first so we can use logging settings
    let settings = config::Config::builder()
        .add_source(config::File::with_name(&cli.config).required(false))
        .add_source(config::Environment::with_prefix("PB").separator("__"))
        .build()?;

    // Initialize tracing: RUST_LOG env > --log-level CLI > config logging.level > "info"
    let log_level = if std::env::var("RUST_LOG").is_ok() {
        // EnvFilter will read RUST_LOG directly
        None
    } else if cli.log_level != "info" {
        // Explicit CLI override
        Some(cli.log_level.clone())
    } else {
        // Fall back to config file
        settings.get_string("logging.level").ok()
    };

    let filter = match log_level {
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        Some(level) => EnvFilter::new(&level),
    };
    fmt().with_env_filter(filter).init();

    // Create shutdown token
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "failed to listen for ctrl_c");
            return;
        }
        tracing::info!("received Ctrl+C, initiating graceful shutdown");
        shutdown_clone.cancel();
    });

    match cli.command {
        Commands::Discover { filter, limit } => {
            commands::discover::run(settings, filter, limit).await?;
        }
        Commands::Ingest {
            tokens,
            parquet,
            clickhouse,
            metrics,
        } => {
            commands::ingest::run(settings, tokens, parquet, clickhouse, metrics, shutdown).await?;
        }
        Commands::Replay { token, at, source } => {
            commands::replay::run(settings, token, at, source).await?;
        }
        Commands::Backfill {
            tokens,
            interval_secs,
            duration_mins,
        } => {
            commands::backfill::run(settings, tokens, interval_secs, duration_mins, shutdown)
                .await?;
        }
    }

    Ok(())
}
