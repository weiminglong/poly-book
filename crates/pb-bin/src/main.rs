use anyhow::Result;
use clap::{Args, Parser, Subcommand};
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
        /// Replay ordering mode: "recv_time" or "exchange_time"
        #[arg(long)]
        mode: String,
        /// Validate against the next checkpoint and persist the validation result
        #[arg(long, default_value_t = false)]
        validate: bool,
    },
    /// Replay stored execution history independently of market-data replay
    ExecutionReplay {
        /// Optional order ID filter
        #[arg(long)]
        order_id: Option<String>,
        /// Start timestamp in microseconds since epoch
        #[arg(long)]
        start: u64,
        /// End timestamp in microseconds since epoch
        #[arg(long)]
        end: u64,
        /// Data source: "parquet" or "clickhouse"
        #[arg(long, default_value = "parquet")]
        source: String,
    },
    /// Append execution events to storage from flags or JSON input
    ExecutionAppend(Box<ExecutionAppendArgs>),
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
    /// Continuously discover and ingest BTC 5-min markets, rotating automatically
    AutoIngest {
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
}

#[derive(Args)]
struct ExecutionAppendArgs {
    /// Data sink: "parquet" or "clickhouse"
    #[arg(long, default_value = "parquet")]
    source: String,
    /// Optional path to a JSON object or array payload
    #[arg(long)]
    json: Option<String>,
    /// Required unless --json is used
    #[arg(long)]
    order_id: Option<String>,
    /// Required unless --json is used
    #[arg(long)]
    event_kind: Option<String>,
    /// Required unless --json is used
    #[arg(long)]
    event_timestamp_us: Option<u64>,
    #[arg(long)]
    asset_id: Option<String>,
    #[arg(long)]
    client_order_id: Option<String>,
    #[arg(long)]
    venue_order_id: Option<String>,
    #[arg(long)]
    side: Option<String>,
    #[arg(long)]
    price: Option<String>,
    #[arg(long)]
    size: Option<String>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    market_data_recv_us: Option<u64>,
    #[arg(long)]
    normalization_done_us: Option<u64>,
    #[arg(long)]
    strategy_decision_us: Option<u64>,
    #[arg(long)]
    order_submit_us: Option<u64>,
    #[arg(long)]
    exchange_ack_us: Option<u64>,
    #[arg(long)]
    exchange_fill_us: Option<u64>,
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
        Commands::Replay {
            token,
            at,
            source,
            mode,
            validate,
        } => {
            commands::replay::run(settings, token, at, source, mode, validate).await?;
        }
        Commands::ExecutionReplay {
            order_id,
            start,
            end,
            source,
        } => {
            commands::execution_replay::run(settings, order_id, start, end, source).await?;
        }
        Commands::ExecutionAppend(args) => {
            let args = *args;
            commands::execution_append::run(
                settings,
                args.source,
                args.json,
                args.order_id,
                args.event_kind,
                args.event_timestamp_us,
                args.asset_id,
                args.client_order_id,
                args.venue_order_id,
                args.side,
                args.price,
                args.size,
                args.status,
                args.reason,
                args.market_data_recv_us,
                args.normalization_done_us,
                args.strategy_decision_us,
                args.order_submit_us,
                args.exchange_ack_us,
                args.exchange_fill_us,
            )
            .await?;
        }
        Commands::Backfill {
            tokens,
            interval_secs,
            duration_mins,
        } => {
            commands::backfill::run(settings, tokens, interval_secs, duration_mins, shutdown)
                .await?;
        }
        Commands::AutoIngest {
            parquet,
            clickhouse,
            metrics,
        } => {
            commands::auto_ingest::run(settings, parquet, clickhouse, metrics, shutdown).await?;
        }
    }

    Ok(())
}
