use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use config::Config;
use pb_replay::EventReader;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::pipeline;

/// Offline demo: replay a committed market-data capture as a simulated live
/// feed behind the full workstation API/UI. No network, no venue dependency —
/// the recorded `PersistedRecord`s stream into the same read model the live
/// pipeline uses, at their original cadence, with timestamps shifted to now so
/// the book reads as current. Historical routes (replay, integrity, query)
/// answer directly from the capture with its original timestamps.
pub async fn run(
    settings: Config,
    data_dir: String,
    speed: f64,
    enable_metrics: bool,
    shutdown: CancellationToken,
    slug_registry: pb_types::SlugRegistry,
) -> Result<()> {
    if !(0.1..=100.0).contains(&speed) {
        bail!("--speed must be between 0.1 and 100");
    }
    let book_events_dir = Path::new(&data_dir).join("book_events");
    if !book_events_dir.is_dir() {
        bail!(
            "no capture found at {data_dir} (expected a {}/ tree). \
             Pass --data-dir or run from the repository root.",
            book_events_dir.display()
        );
    }

    if enable_metrics {
        pipeline::start_metrics_server(&settings).await?;
    }

    // Discover assets and the capture window from the partition layout:
    // {data_dir}/book_events/YYYY/MM/DD/HH/{asset}_{first_ts}_{hash}_{len}.parquet
    let capture = discover_capture(&book_events_dir)?;
    tracing::info!(
        assets = capture.assets.len(),
        window_start_us = capture.start_us,
        window_end_us = capture.end_us,
        "demo capture discovered"
    );

    // Load every asset's market-data window through the same reader the
    // replay engine uses.
    let reader = pb_replay::ParquetReader::new(&data_dir);
    let mut records: Vec<pb_types::PersistedRecord> = Vec::new();
    for asset in &capture.assets {
        let asset_id = pb_types::AssetId::from(asset.as_str());
        let window = reader
            .read_market_data(&asset_id, capture.start_us, capture.end_us)
            .await
            .with_context(|| format!("reading capture for {asset}"))?;
        records.extend(
            window
                .book_events
                .into_iter()
                .map(pb_types::PersistedRecord::Book),
        );
        records.extend(
            window
                .trade_events
                .into_iter()
                .map(pb_types::PersistedRecord::Trade),
        );
        records.extend(
            window
                .ingest_events
                .into_iter()
                .map(pb_types::PersistedRecord::Ingest),
        );
    }
    if records.is_empty() {
        bail!("capture at {data_dir} contains no market-data events");
    }
    // Deterministic stream order across assets: arrival order (ingest ordinal)
    // within the recv-timestamp clock, matching the replay engine's ordering.
    records.sort_by_key(|r| {
        let recv = r.recv_timestamp_us().unwrap_or(0);
        let ordinal = match r {
            pb_types::PersistedRecord::Book(e) => e.provenance.ingest_ordinal,
            pb_types::PersistedRecord::Trade(e) => e.provenance.ingest_ordinal,
            pb_types::PersistedRecord::Ingest(e) => e.provenance.ingest_ordinal,
            _ => None,
        };
        (recv, ordinal.unwrap_or(u64::MAX))
    });
    let event_count = records.len();

    let api_listen_addr: SocketAddr = settings
        .get_string("api.listen_addr")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
        .parse()?;
    let max_depth = settings.get_int("api.max_depth").unwrap_or(200).max(1) as usize;
    let default_depth =
        (settings.get_int("api.default_depth").unwrap_or(20).max(1) as usize).min(max_depth);
    let stale_after_secs = settings
        .get_int("api.stale_after_secs")
        .unwrap_or(15)
        .max(1) as u64;

    let live = pb_api::LiveReadModel::new(pb_api::FeedMode::FixedTokens);
    let (event_tx, event_rx) = mpsc::channel::<pb_types::PersistedRecord>(2_048);
    let broadcast = pb_api::BookBroadcast::new();
    let consumer_handle = live.spawn_consumer_with_broadcast(
        event_rx,
        broadcast.clone(),
        default_depth,
        shutdown.child_token(),
    );
    broadcast.set_active_assets(&capture.assets);
    live.set_active_assets(capture.assets.clone()).await;
    live.mark_hydrated().await;

    // Replay-to-live bridge: stream the capture into the read model at its
    // original cadence (divided by --speed), looping forever. Each pass
    // re-anchors to the wall clock and shifts every event's provenance
    // timestamps by the anchor delta, so the "live" surfaces tick like a
    // current market while storage keeps the original capture timestamps.
    // Pre-seed every asset with its first captured venue snapshot so the live
    // surfaces answer immediately. Without this, an auto-rotate capture keeps
    // later-subscribing assets returning 503 until the playback clock reaches
    // their subscribe point, which can be minutes into the stream.
    let seed_records = first_snapshot_per_asset(&records);
    let seed_now = super::market_discovery::now_us();
    for record in seed_records {
        let mut seeded = record.clone();
        if let Some(provenance) = seeded.provenance_mut() {
            provenance.exchange_timestamp_us = provenance
                .exchange_timestamp_us
                .saturating_add(seed_now.saturating_sub(provenance.recv_timestamp_us));
            provenance.recv_timestamp_us = seed_now;
        }
        if event_tx.send(seeded).await.is_err() {
            bail!("read-model consumer stopped during demo seeding");
        }
    }

    let feeder_records = records;
    let feeder_shutdown = shutdown.child_token();
    let feeder_start = capture.start_us;
    let feeder_tx = event_tx.clone();
    let feeder_handle = tokio::spawn(async move {
        loop {
            let loop_anchor_us = super::market_discovery::now_us();
            for record in &feeder_records {
                let original_recv = record.recv_timestamp_us().unwrap_or(feeder_start);
                let offset_us = original_recv.saturating_sub(feeder_start);
                let scaled_offset_us = (offset_us as f64 / speed) as u64;
                let emit_at_us = loop_anchor_us.saturating_add(scaled_offset_us);
                let now = super::market_discovery::now_us();
                if emit_at_us > now {
                    tokio::select! {
                        _ = feeder_shutdown.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_micros(emit_at_us - now)) => {}
                    }
                } else if feeder_shutdown.is_cancelled() {
                    return;
                }
                // Shift both provenance clocks so the event reads as emitted
                // now; exchange keeps its original skew relative to recv.
                let shift_us = emit_at_us.saturating_sub(original_recv);
                let mut shifted = record.clone();
                if let Some(provenance) = shifted.provenance_mut() {
                    provenance.recv_timestamp_us =
                        provenance.recv_timestamp_us.saturating_add(shift_us);
                    provenance.exchange_timestamp_us =
                        provenance.exchange_timestamp_us.saturating_add(shift_us);
                }
                if feeder_tx.send(shifted).await.is_err() {
                    return; // consumer stopped (shutdown)
                }
            }
            tracing::info!("demo capture exhausted; looping from the start");
        }
    });

    // Historical services read the capture directly — always the Parquet
    // backend at --data-dir, never the configured storage paths, so the demo
    // cannot accidentally serve (or require) live-pipeline data.
    let replay_service =
        pb_service::AnyReplayService::Parquet(pb_service::ParquetReplayService::new(&data_dir));
    let integrity_service = pb_service::AnyIntegrityService::Parquet(
        pb_service::ParquetIntegrityService::new(&data_dir),
    );
    let execution_service = pb_service::AnyExecutionService::Parquet(
        pb_service::ParquetExecutionService::new(&data_dir),
    );
    // The SQL workbench is ClickHouse-backed and stays disabled in the demo.
    let query_service = None;
    let (query_max_rows, query_timeout_secs) = pipeline::query_config_from_settings(&settings);
    let auth_token = pipeline::api_auth_token_from_settings(&settings);
    pipeline::validate_api_auth_boundary(
        api_listen_addr,
        false,
        api_listen_addr,
        auth_token.as_deref(),
    )?;

    let state = pb_api::AppState {
        live,
        config: pb_api::ApiConfig {
            parquet_base_path: data_dir.clone(),
            default_depth,
            max_depth,
            stale_after_secs,
            query_max_rows,
            query_timeout_secs,
            http_request_timeout_secs: pipeline::cfg_int_min(
                &settings,
                "api.http_request_timeout_secs",
                pb_api::DEFAULT_HTTP_REQUEST_TIMEOUT_SECS as i64,
                1,
            ) as u64,
            auth_token,
            static_assets_dir: settings
                .get_string("api.static_assets_dir")
                .ok()
                .filter(|d| !d.is_empty()),
        },
        broadcast: Some(broadcast.clone()),
        slug_registry,
        replay_service,
        integrity_service,
        execution_service,
        query_service,
        wal_lag_bytes: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        needs_resync: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let listener = tokio::net::TcpListener::bind(api_listen_addr).await?;

    print_cheat_sheet(&capture, event_count, api_listen_addr);

    let serve_result = pb_api::serve(listener, state, shutdown.child_token()).await;
    drop(event_tx);
    shutdown.cancel();
    pipeline::shutdown_handles(vec![feeder_handle, consumer_handle], "demo task").await;
    serve_result?;
    Ok(())
}

struct Capture {
    assets: Vec<String>,
    start_us: u64,
    end_us: u64,
}

/// Extract each asset's first complete venue snapshot from the arrival-ordered
/// record stream — the first contiguous run of `Snapshot`-kind book events for
/// that asset — plus the first delta that follows it. The read model holds a
/// snapshot burst pending until a boundary record arrives (that is how it
/// groups per-level snapshot events into one atomic book apply), so the
/// trailing delta is what materializes the seeded book.
fn first_snapshot_per_asset(
    records: &[pb_types::PersistedRecord],
) -> Vec<&pb_types::PersistedRecord> {
    use std::collections::HashMap;

    #[derive(Clone, Copy, PartialEq)]
    enum SeedState {
        Waiting,
        InSnapshot,
        Done,
    }

    let mut state: HashMap<&str, SeedState> = HashMap::new();
    let mut seeds = Vec::new();
    for record in records {
        let pb_types::PersistedRecord::Book(event) = record else {
            continue;
        };
        let asset = event.asset_id.as_str();
        let entry = state.entry(asset).or_insert(SeedState::Waiting);
        match (*entry, event.kind) {
            (SeedState::Waiting, pb_types::BookEventKind::Snapshot) => {
                *entry = SeedState::InSnapshot;
                seeds.push(record);
            }
            (SeedState::InSnapshot, pb_types::BookEventKind::Snapshot) => {
                seeds.push(record);
            }
            (SeedState::InSnapshot, pb_types::BookEventKind::Delta) => {
                // Boundary event: materializes the pending snapshot group.
                seeds.push(record);
                *entry = SeedState::Done;
            }
            _ => {}
        }
    }
    seeds
}

/// Walk the hour-partitioned Parquet tree and derive assets + the time window
/// from file names ({asset}_{first_ts}_{content_hash}_{len}.parquet), without
/// reading any Parquet.
fn discover_capture(book_events_dir: &Path) -> Result<Capture> {
    let mut assets = std::collections::BTreeSet::new();
    let mut min_ts = u64::MAX;
    let mut max_ts = 0u64;
    let mut stack = vec![book_events_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".parquet") else {
                continue;
            };
            // {asset}_{first_ts}_{hash}_{len}: split from the right so asset
            // ids containing no underscores stay intact.
            let parts: Vec<&str> = stem.rsplitn(4, '_').collect();
            if parts.len() != 4 {
                continue;
            }
            let (asset, first_ts) = (parts[3], parts[2]);
            let Ok(ts) = first_ts.parse::<u64>() else {
                continue;
            };
            assets.insert(asset.to_string());
            min_ts = min_ts.min(ts);
            max_ts = max_ts.max(ts);
        }
    }
    if assets.is_empty() {
        bail!("no Parquet files found under {}", book_events_dir.display());
    }
    Ok(Capture {
        assets: assets.into_iter().collect(),
        // Pad generously: file names carry each file's FIRST timestamp, so the
        // last file's events extend past max_ts. An hour of slack on both
        // sides keeps hour-directory enumeration bounded while covering the
        // full capture.
        start_us: min_ts.saturating_sub(3_600_000_000),
        end_us: max_ts.saturating_add(3_600_000_000),
    })
}

fn print_cheat_sheet(capture: &Capture, event_count: usize, addr: SocketAddr) {
    let mid_us = capture.start_us + (capture.end_us - capture.start_us) / 2;
    let asset = capture.assets.first().cloned().unwrap_or_default();
    println!("\n=== poly-book offline demo ===");
    println!("workstation UI + API:  http://{addr}/  (UI requires the Docker image or api.static_assets_dir)");
    println!("assets: {}", capture.assets.join(", "));
    println!(
        "capture window (us): {} .. {}  ({} events, replayed on a loop)",
        capture.start_us, capture.end_us, event_count
    );
    println!("\ncopy-paste examples against the capture:");
    println!("  curl 'http://{addr}/api/v1/orderbooks/{asset}/snapshot'");
    println!("  curl 'http://{addr}/api/v1/replay/reconstruct?asset_id={asset}&at_us={mid_us}&mode=recv_time'");
    println!(
        "  curl 'http://{addr}/api/v1/integrity/summary?asset_id={asset}&start_us={}&end_us={}'",
        capture.start_us, capture.end_us
    );
    println!("==============================\n");
}
