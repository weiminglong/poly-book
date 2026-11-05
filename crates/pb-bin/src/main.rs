//! CLI entrypoint for the poly-book workspace. See [README](../README.md).

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{fmt, EnvFilter};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod commands;
mod settings;

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

    /// Log level (trace, debug, info, warn, error); overrides logging.level
    #[arg(long)]
    log_level: Option<String>,

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
    /// Start live orderbook ingestion (feed -> WAL -> Parquet) for fixed
    /// tokens or the rotating BTC 5-minute market
    Ingest {
        /// Comma-separated token IDs to subscribe to
        #[arg(long, conflicts_with = "auto_rotate")]
        tokens: Option<String>,
        /// Follow the live BTC 5-minute market, rotating every 5 minutes
        #[arg(long, default_value_t = false)]
        auto_rotate: bool,
        /// Enable Parquet storage (use --parquet=false to disable)
        #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_value_t = true, default_missing_value = "true")]
        parquet: bool,
        /// Enable metrics server (use --metrics=false to disable)
        #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_value_t = true, default_missing_value = "true")]
        metrics: bool,
        /// Start as a hot standby: wait for the active writer's WAL lock and
        /// auto-promote when it is released (writer failover).
        #[arg(long, default_value_t = false)]
        standby: bool,
    },
    /// Replay historical orderbook state at a specific timestamp
    Replay {
        /// Token ID to replay
        #[arg(long)]
        token: String,
        /// Target timestamp in microseconds since epoch
        #[arg(long)]
        at: u64,
        /// Replay ordering mode: "recv_time" or "exchange_time"
        #[arg(long, default_value = "recv_time")]
        mode: String,
        /// Validate against the next checkpoint and persist the validation result
        #[arg(long, default_value_t = false)]
        validate: bool,
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
    /// Preflight checklist: validate config and probe every external
    /// dependency (feed endpoints, storage paths, ports); exits non-zero on
    /// failure so it can gate deploys
    Doctor {
        /// Skip feed reachability probes (offline/CI use)
        #[arg(long, default_value_t = false)]
        skip_network: bool,
    },
    /// Replay a committed capture as a simulated live feed behind the full
    /// workstation API/UI — fully offline, no venue dependency
    Demo {
        /// Directory containing the capture's Parquet tree
        #[arg(long, default_value = "demo/data")]
        data_dir: String,
        /// Playback speed multiplier (0.1–100)
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// Enable metrics server (use --metrics=false to disable)
        #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_value_t = true, default_missing_value = "true")]
        metrics: bool,
    },
    /// Start the read-only API server with a live feed and replay access
    ServeApi {
        /// Comma-separated token IDs to subscribe to
        #[arg(long, conflicts_with = "auto_rotate")]
        tokens: Option<String>,
        /// Automatically rotate to the live BTC 5-minute market
        #[arg(long, default_value_t = false)]
        auto_rotate: bool,
        /// Enable metrics server (use --metrics=false to disable)
        #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_value_t = true, default_missing_value = "true")]
        metrics: bool,
    },
    /// Start the read-only serve runtime (WAL reader + checkpoint hydration + HTTP/WS)
    Serve {
        /// Comma-separated token IDs to serve. Omit to follow whatever the
        /// ingest process writes to the WAL (e.g. the rotating market that
        /// `ingest --auto-rotate` tracks).
        #[arg(long)]
        tokens: Option<String>,
        /// Enable metrics server (use --metrics=false to disable)
        #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_value_t = true, default_missing_value = "true")]
        metrics: bool,
    },
    /// Rebuild Parquet storage partitions from the durable WAL (offline recovery)
    Reconcile,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install a process-level rustls crypto provider before anything opens a
    // TLS connection. The dependency graph links two providers (`ring` and
    // `aws-lc-rs` via the metrics exporter), and rustls panics on first use if
    // no default is selected. pb-feed pins its own provider for the WS
    // connector; this covers every other rustls consumer in the process.
    // Err(_) means a provider is already installed, which is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();

    // Load config first so we can use logging settings. An explicitly-passed
    // config path that does not exist is a hard error — silently falling back to
    // all-defaults (the old `required(false)` behavior) hides misconfiguration
    // in production. The repo default path is allowed to be
    // absent so the binary still runs from an arbitrary CWD.
    const DEFAULT_CONFIG_PATH: &str = "config/default.toml";
    let config_is_default = cli.config == DEFAULT_CONFIG_PATH;
    if !config_is_default && !std::path::Path::new(&cli.config).exists() {
        anyhow::bail!("config file not found: {}", cli.config);
    }
    let settings = settings::Settings::load(&cli.config, !config_is_default)?;

    // Initialize tracing: RUST_LOG env > --log-level CLI > config logging.level
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    } else {
        EnvFilter::new(cli.log_level.as_deref().unwrap_or(&settings.logging.level))
    };
    let builder = fmt().with_env_filter(filter);
    if settings.logging.format.eq_ignore_ascii_case("json") {
        builder.json().init();
    } else {
        builder.init();
    }

    // Create shared slug registry
    let slug_registry = pb_types::SlugRegistry::new();

    // Create shutdown token
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    // Register the SIGTERM handler synchronously BEFORE spawning, so a failed
    // registration is a clean startup error (non-zero exit) rather than a panic
    // buried in a detached task that would silently break all shutdown signaling
    //. The registered Signal is then moved into the listener task.
    #[cfg(unix)]
    let mut sigterm = {
        use tokio::signal::unix::{signal, SignalKind};
        signal(SignalKind::terminate())
            .map_err(|e| anyhow::anyhow!("failed to register SIGTERM handler: {e}"))?
    };
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
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
            auto_rotate,
            parquet,
            metrics,
            standby,
        } => {
            let plan = commands::feed::FeedPlan::from_flags(tokens, auto_rotate)?;
            commands::ingest::run(
                settings,
                plan,
                parquet,
                metrics,
                standby,
                shutdown,
                slug_registry,
            )
            .await?;
        }
        Commands::Replay {
            token,
            at,
            mode,
            validate,
        } => {
            commands::replay::run(settings, token, at, mode, validate).await?;
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
        Commands::Doctor { skip_network } => {
            commands::doctor::run(settings, skip_network).await?;
        }
        Commands::Demo {
            data_dir,
            speed,
            metrics,
        } => {
            commands::demo::run(settings, data_dir, speed, metrics, shutdown, slug_registry)
                .await?;
        }
        Commands::ServeApi {
            tokens,
            auto_rotate,
            metrics,
        } => {
            let plan = commands::feed::FeedPlan::from_flags(tokens, auto_rotate)?;
            commands::serve_api::run(settings, plan, metrics, shutdown, slug_registry).await?;
        }
        Commands::Serve { tokens, metrics } => {
            commands::serve::run(settings, tokens, metrics, shutdown, slug_registry).await?;
        }
        Commands::Reconcile => {
            commands::reconcile::run(settings).await?;
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
    fn parse_doctor_defaults() {
        let cli = Cli::try_parse_from(["poly-book", "doctor"]).unwrap();
        match cli.command {
            Commands::Doctor { skip_network } => assert!(!skip_network),
            _ => panic!("expected Doctor"),
        }
    }

    #[test]
    fn parse_doctor_skip_network() {
        let cli = Cli::try_parse_from(["poly-book", "doctor", "--skip-network"]).unwrap();
        match cli.command {
            Commands::Doctor { skip_network } => assert!(skip_network),
            _ => panic!("expected Doctor"),
        }
    }

    #[test]
    fn parse_demo_defaults() {
        let cli = Cli::try_parse_from(["poly-book", "demo"]).unwrap();
        match cli.command {
            Commands::Demo {
                data_dir,
                speed,
                metrics,
            } => {
                assert_eq!(data_dir, "demo/data");
                assert_eq!(speed, 1.0);
                assert!(metrics);
            }
            _ => panic!("expected Demo"),
        }
    }

    #[test]
    fn parse_demo_with_args() {
        let cli = Cli::try_parse_from([
            "poly-book",
            "demo",
            "--data-dir",
            "/tmp/capture",
            "--speed",
            "4",
            "--metrics=false",
        ])
        .unwrap();
        match cli.command {
            Commands::Demo {
                data_dir,
                speed,
                metrics,
            } => {
                assert_eq!(data_dir, "/tmp/capture");
                assert_eq!(speed, 4.0);
                assert!(!metrics);
            }
            _ => panic!("expected Demo"),
        }
    }

    #[test]
    fn parse_ingest_defaults() {
        let cli = Cli::try_parse_from(["poly-book", "ingest"]).unwrap();
        match cli.command {
            Commands::Ingest {
                tokens,
                auto_rotate,
                parquet,
                metrics,
                standby,
            } => {
                assert!(tokens.is_none());
                assert!(!auto_rotate);
                assert!(parquet);
                assert!(metrics);
                assert!(!standby);
            }
            _ => panic!("expected Ingest"),
        }
    }

    #[test]
    fn parse_ingest_standby_flag() {
        let cli = Cli::try_parse_from(["poly-book", "ingest", "--standby"]).unwrap();
        match cli.command {
            Commands::Ingest { standby, .. } => assert!(standby),
            _ => panic!("expected Ingest"),
        }
    }

    #[test]
    fn parse_ingest_with_tokens() {
        let cli = Cli::try_parse_from(["poly-book", "ingest", "--tokens", "tok1,tok2"]).unwrap();
        match cli.command {
            Commands::Ingest { tokens, .. } => {
                assert_eq!(tokens.as_deref(), Some("tok1,tok2"));
            }
            _ => panic!("expected Ingest"),
        }
    }

    #[test]
    fn ingest_toggles_can_be_disabled_and_presence_still_enables() {
        // Default-true toggles must be disable-able with `=false`, while a
        // bare `--metrics` still enables via default_missing_value.
        let cli = Cli::try_parse_from([
            "poly-book",
            "ingest",
            "--tokens",
            "tok1",
            "--metrics",
            "--parquet=false",
        ])
        .unwrap();
        match cli.command {
            Commands::Ingest {
                metrics, parquet, ..
            } => {
                assert!(metrics, "bare --metrics should enable metrics");
                assert!(!parquet, "--parquet=false should disable parquet");
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
                mode,
                validate,
            } => {
                assert_eq!(token, "tok1");
                assert_eq!(at, 1_000_000);
                assert_eq!(mode, "recv_time");
                assert!(!validate);
            }
            _ => panic!("expected Replay"),
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
    fn parse_ingest_auto_rotate() {
        let cli = Cli::try_parse_from(["poly-book", "ingest", "--auto-rotate"]).unwrap();
        match cli.command {
            Commands::Ingest {
                tokens,
                auto_rotate,
                ..
            } => {
                assert!(tokens.is_none());
                assert!(auto_rotate);
            }
            _ => panic!("expected Ingest"),
        }
        // --tokens and --auto-rotate are mutually exclusive.
        assert!(
            Cli::try_parse_from(["poly-book", "ingest", "--tokens", "tok1", "--auto-rotate"])
                .is_err()
        );
    }

    #[test]
    fn parse_serve_api() {
        let cli = Cli::try_parse_from(["poly-book", "serve-api", "--auto-rotate"]).unwrap();
        match cli.command {
            Commands::ServeApi {
                tokens,
                auto_rotate,
                ..
            } => {
                assert!(tokens.is_none());
                assert!(auto_rotate);
            }
            _ => panic!("expected ServeApi"),
        }
    }

    #[test]
    fn parse_serve() {
        let cli = Cli::try_parse_from(["poly-book", "serve", "--tokens", "tok1,tok2"]).unwrap();
        match cli.command {
            Commands::Serve { tokens, metrics } => {
                assert_eq!(tokens.as_deref(), Some("tok1,tok2"));
                assert!(metrics);
            }
            _ => panic!("expected Serve"),
        }
    }

    #[test]
    fn parse_serve_without_tokens_follows_wal() {
        let cli = Cli::try_parse_from(["poly-book", "serve"]).unwrap();
        match cli.command {
            Commands::Serve { tokens, .. } => assert!(tokens.is_none()),
            _ => panic!("expected Serve"),
        }
    }

    #[test]
    fn parse_reconcile() {
        let cli = Cli::try_parse_from(["poly-book", "reconcile"]).unwrap();
        assert!(matches!(cli.command, Commands::Reconcile));
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
        // --token and --at are required; --mode defaults to recv_time
        let result = Cli::try_parse_from(["poly-book", "replay", "--token", "tok1"]);
        assert!(result.is_err());
        let cli =
            Cli::try_parse_from(["poly-book", "replay", "--token", "tok1", "--at", "5"]).unwrap();
        match cli.command {
            Commands::Replay { mode, .. } => assert_eq!(mode, "recv_time"),
            _ => panic!("expected Replay"),
        }
    }

    #[test]
    fn parse_backfill_missing_tokens_fails() {
        let result = Cli::try_parse_from(["poly-book", "backfill"]);
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
        let cli = Cli::try_parse_from(["poly-book", "--log-level", "debug", "discover"]).unwrap();
        assert_eq!(cli.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn parse_global_defaults() {
        let cli = Cli::try_parse_from(["poly-book", "discover"]).unwrap();
        assert_eq!(cli.config, "config/default.toml");
        assert!(cli.log_level.is_none());
    }
}
