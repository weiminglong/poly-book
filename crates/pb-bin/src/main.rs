//! CLI entrypoint for the poly-book workspace. See [README](../README.md).

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{fmt, EnvFilter};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
    ExecutionAppend(Box<commands::execution_append::ExecutionAppendArgs>),
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
    /// Start the read-only API server with a live feed and replay access
    ServeApi {
        /// Comma-separated token IDs to subscribe to
        #[arg(long)]
        tokens: Option<String>,
        /// Automatically rotate to the live BTC 5-minute market
        #[arg(long, default_value_t = false)]
        auto_rotate: bool,
        /// Enable metrics server
        #[arg(long, default_value_t = true)]
        metrics: bool,
    },
    /// Start the read-only serve runtime (WAL reader + checkpoint hydration + HTTP/WS)
    Serve {
        /// Comma-separated token IDs to serve
        #[arg(long)]
        tokens: String,
        /// Enable metrics server
        #[arg(long, default_value_t = true)]
        metrics: bool,
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

    // Create shared slug registry
    let slug_registry = pb_types::SlugRegistry::new();

    // Create shutdown token
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
            tokio::select! {
                result = ctrl_c => {
                    if let Err(e) = result {
                        tracing::error!(error = %e, "failed to listen for ctrl_c");
                        return;
                    }
                    tracing::info!("received SIGINT, initiating graceful shutdown");
                }
                _ = sigterm.recv() => {
                    tracing::info!("received SIGTERM, initiating graceful shutdown");
                }
            }
        }

        #[cfg(not(unix))]
        {
            if let Err(e) = ctrl_c.await {
                tracing::error!(error = %e, "failed to listen for ctrl_c");
                return;
            }
            tracing::info!("received Ctrl+C, initiating graceful shutdown");
        }

        shutdown_clone.cancel();
    });

    match cli.command {
        Commands::Discover { filter, limit } => {
            commands::discover::run(settings, filter, limit, slug_registry).await?;
        }
        Commands::Ingest {
            tokens,
            parquet,
            clickhouse,
            metrics,
        } => {
            commands::ingest::run(
                settings,
                tokens,
                parquet,
                clickhouse,
                metrics,
                shutdown,
                slug_registry,
            )
            .await?;
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
            commands::execution_append::run(settings, *args).await?;
        }
        Commands::Backfill {
            tokens,
            interval_secs,
            duration_mins,
        } => {
            commands::backfill::run(
                settings,
                tokens,
                interval_secs,
                duration_mins,
                shutdown,
                slug_registry,
            )
            .await?;
        }
        Commands::AutoIngest {
            parquet,
            clickhouse,
            metrics,
        } => {
            commands::auto_ingest::run(
                settings,
                parquet,
                clickhouse,
                metrics,
                shutdown,
                slug_registry,
            )
            .await?;
        }
        Commands::ServeApi {
            tokens,
            auto_rotate,
            metrics,
        } => {
            commands::serve_api::run(
                settings,
                tokens,
                auto_rotate,
                metrics,
                shutdown,
                slug_registry,
            )
            .await?;
        }
        Commands::Serve { tokens, metrics } => {
            commands::serve::run(settings, tokens, metrics, shutdown, slug_registry).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    // --- CLI parsing: valid subcommands ---

    #[test]
    fn parse_discover_defaults() {
        let cli = Cli::try_parse_from(["poly-book", "discover"]).unwrap();
        match cli.command {
            Commands::Discover { filter, limit } => {
                assert!(filter.is_none());
                assert_eq!(limit, 500);
            }
            _ => panic!("expected Discover"),
        }
    }

    #[test]
    fn parse_discover_with_args() {
        let cli =
            Cli::try_parse_from(["poly-book", "discover", "--filter", "BTC", "--limit", "100"])
                .unwrap();
        match cli.command {
            Commands::Discover { filter, limit } => {
                assert_eq!(filter.as_deref(), Some("BTC"));
                assert_eq!(limit, 100);
            }
            _ => panic!("expected Discover"),
        }
    }

    #[test]
    fn parse_ingest_defaults() {
        let cli = Cli::try_parse_from(["poly-book", "ingest"]).unwrap();
        match cli.command {
            Commands::Ingest {
                tokens,
                parquet,
                clickhouse,
                metrics,
            } => {
                assert!(tokens.is_none());
                assert!(parquet);
                assert!(!clickhouse);
                assert!(metrics);
            }
            _ => panic!("expected Ingest"),
        }
    }

    #[test]
    fn parse_ingest_with_tokens() {
        let cli = Cli::try_parse_from([
            "poly-book",
            "ingest",
            "--tokens",
            "tok1,tok2",
            "--clickhouse",
        ])
        .unwrap();
        match cli.command {
            Commands::Ingest {
                tokens,
                clickhouse,
                ..
            } => {
                assert_eq!(tokens.as_deref(), Some("tok1,tok2"));
                assert!(clickhouse);
            }
            _ => panic!("expected Ingest"),
        }
    }

    #[test]
    fn parse_replay_requires_all_args() {
        let cli = Cli::try_parse_from([
            "poly-book",
            "replay",
            "--token",
            "tok1",
            "--at",
            "1000000",
            "--mode",
            "recv_time",
        ])
        .unwrap();
        match cli.command {
            Commands::Replay {
                token,
                at,
                source,
                mode,
                validate,
            } => {
                assert_eq!(token, "tok1");
                assert_eq!(at, 1_000_000);
                assert_eq!(source, "parquet");
                assert_eq!(mode, "recv_time");
                assert!(!validate);
            }
            _ => panic!("expected Replay"),
        }
    }

    #[test]
    fn parse_execution_replay() {
        let cli = Cli::try_parse_from([
            "poly-book",
            "execution-replay",
            "--start",
            "100",
            "--end",
            "200",
        ])
        .unwrap();
        match cli.command {
            Commands::ExecutionReplay {
                order_id,
                start,
                end,
                source,
            } => {
                assert!(order_id.is_none());
                assert_eq!(start, 100);
                assert_eq!(end, 200);
                assert_eq!(source, "parquet");
            }
            _ => panic!("expected ExecutionReplay"),
        }
    }

    #[test]
    fn parse_backfill() {
        let cli = Cli::try_parse_from([
            "poly-book",
            "backfill",
            "--tokens",
            "tok1,tok2",
            "--interval-secs",
            "30",
        ])
        .unwrap();
        match cli.command {
            Commands::Backfill {
                tokens,
                interval_secs,
                duration_mins,
            } => {
                assert_eq!(tokens, "tok1,tok2");
                assert_eq!(interval_secs, 30);
                assert_eq!(duration_mins, 0);
            }
            _ => panic!("expected Backfill"),
        }
    }

    #[test]
    fn parse_auto_ingest() {
        let cli = Cli::try_parse_from(["poly-book", "auto-ingest"]).unwrap();
        match cli.command {
            Commands::AutoIngest {
                parquet,
                clickhouse,
                metrics,
            } => {
                assert!(parquet);
                assert!(!clickhouse);
                assert!(metrics);
            }
            _ => panic!("expected AutoIngest"),
        }
    }

    #[test]
    fn parse_serve_api() {
        let cli =
            Cli::try_parse_from(["poly-book", "serve-api", "--tokens", "tok1", "--auto-rotate"])
                .unwrap();
        match cli.command {
            Commands::ServeApi {
                tokens,
                auto_rotate,
                ..
            } => {
                assert_eq!(tokens.as_deref(), Some("tok1"));
                assert!(auto_rotate);
            }
            _ => panic!("expected ServeApi"),
        }
    }

    #[test]
    fn parse_serve() {
        let cli =
            Cli::try_parse_from(["poly-book", "serve", "--tokens", "tok1,tok2"]).unwrap();
        match cli.command {
            Commands::Serve { tokens, metrics } => {
                assert_eq!(tokens, "tok1,tok2");
                assert!(metrics);
            }
            _ => panic!("expected Serve"),
        }
    }

    // --- CLI parsing: invalid / missing args ---

    #[test]
    fn parse_no_subcommand_fails() {
        let result = Cli::try_parse_from(["poly-book"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_unknown_subcommand_fails() {
        let result = Cli::try_parse_from(["poly-book", "frobnicate"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_replay_missing_required_args_fails() {
        // --token and --at and --mode are required
        let result = Cli::try_parse_from(["poly-book", "replay", "--token", "tok1"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_backfill_missing_tokens_fails() {
        let result = Cli::try_parse_from(["poly-book", "backfill"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_serve_missing_tokens_fails() {
        let result = Cli::try_parse_from(["poly-book", "serve"]);
        assert!(result.is_err());
    }

    // --- Global CLI flags ---

    #[test]
    fn parse_global_config_flag() {
        let cli =
            Cli::try_parse_from(["poly-book", "--config", "/tmp/alt.toml", "discover"]).unwrap();
        assert_eq!(cli.config, "/tmp/alt.toml");
    }

    #[test]
    fn parse_global_log_level_flag() {
        let cli =
            Cli::try_parse_from(["poly-book", "--log-level", "debug", "discover"]).unwrap();
        assert_eq!(cli.log_level, "debug");
    }

    #[test]
    fn parse_global_defaults() {
        let cli = Cli::try_parse_from(["poly-book", "discover"]).unwrap();
        assert_eq!(cli.config, "config/default.toml");
        assert_eq!(cli.log_level, "info");
    }
}
