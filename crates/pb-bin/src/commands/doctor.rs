use std::fmt;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::Result;
use object_store::ObjectStoreExt;

use crate::settings::Settings;

/// Preflight checklist: validate the effective configuration and probe every
/// external dependency a poly-book process needs, printing a pass/warn/fail
/// table and exiting non-zero on failure. Doubles as a deploy gate
/// (`poly-book doctor && poly-book ingest`).
pub async fn run(settings: Settings, skip_network: bool) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    checks.push(check_parquet_path(&settings).await);
    checks.push(check_wal_dir(&settings));

    if skip_network {
        checks.push(Check::warn(
            "network",
            "skipped (--skip-network); feed reachability not verified",
        ));
    } else {
        checks.push(check_http("rest", &settings.feed.rest_url).await);
        checks.push(check_http("gamma", &settings.feed.gamma_url).await);
        checks.push(check_websocket(&settings.feed.ws_url).await);
    }

    checks.push(check_port("api port", settings.api.listen_addr).await);
    checks.push(check_port("metrics port", settings.metrics.listen_addr).await);

    let mut warns = 0usize;
    let mut fails = 0usize;
    println!("\npoly-book doctor\n");
    for check in &checks {
        println!("  {:<13} {:<5} {}", check.name, check.status, check.detail);
        match check.status {
            Status::Warn => warns += 1,
            Status::Fail => fails += 1,
            Status::Pass => {}
        }
    }
    println!();
    if fails > 0 {
        println!("status: FAILED ({fails} failure(s), {warns} warning(s))");
        std::process::exit(1);
    }
    println!("status: ok ({warns} warning(s))");
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Pass,
    Warn,
    Fail,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Status::Pass => "pass",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
        })
    }
}

struct Check {
    name: &'static str,
    status: Status,
    detail: String,
}

impl Check {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Pass,
            detail: detail.into(),
        }
    }
    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Warn,
            detail: detail.into(),
        }
    }
    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Fail,
            detail: detail.into(),
        }
    }
}

async fn check_parquet_path(settings: &Settings) -> Check {
    let base = settings.storage.parquet_base_path.clone();
    let (store, prefix) = match super::pipeline::build_object_store(&base) {
        Ok(result) => result,
        Err(error) => {
            return Check::fail("parquet", format!("{base} configuration failed: {error}"));
        }
    };
    let nonce = format!(
        ".doctor-write-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let encoded = if prefix.is_empty() {
        nonce
    } else {
        format!("{prefix}/{nonce}")
    };
    let probe = match object_store::path::Path::parse(&encoded) {
        Ok(path) => path,
        Err(error) => {
            return Check::fail(
                "parquet",
                format!("{base} has invalid object prefix: {error}"),
            );
        }
    };

    let probe_result = tokio::time::timeout(Duration::from_secs(10), async {
        store
            .put(
                &probe,
                object_store::PutPayload::from_static(b"poly-book-doctor"),
            )
            .await?;
        let bytes = store.get(&probe).await?.bytes().await?;
        if bytes.as_ref() != b"poly-book-doctor" {
            return Err(object_store::Error::Generic {
                store: "doctor",
                source: Box::new(std::io::Error::other(
                    "storage probe returned different bytes",
                )),
            });
        }
        store.delete(&probe).await?;
        Ok::<(), object_store::Error>(())
    })
    .await;

    match probe_result {
        Ok(Ok(())) => Check::pass("parquet", format!("{base} write/read/delete probe passed")),
        Ok(Err(error)) => Check::fail(
            "parquet",
            format!(
                "{base} probe failed: {}",
                super::pipeline::object_store_error_summary(&error)
            ),
        ),
        Err(_) => Check::fail("parquet", format!("{base} probe timed out after 10s")),
    }
}

fn check_wal_dir(settings: &Settings) -> Check {
    let path = settings.wal.base_path.as_path();
    let base = path.display();
    if !path.is_dir() {
        return Check::warn(
            "wal",
            format!("{base} does not exist yet (created by ingest)"),
        );
    }
    let mut segments = 0usize;
    let mut consumers = 0usize;
    let mut newest: Option<std::time::SystemTime> = None;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("segment_") && name.ends_with(".wal") {
                segments += 1;
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        newest = Some(
                            newest.map_or(modified, |n: std::time::SystemTime| n.max(modified)),
                        );
                    }
                }
            } else if name.starts_with("consumer_") && name.ends_with(".pos") {
                consumers += 1;
            }
        }
    }
    let age = newest
        .and_then(|n| n.elapsed().ok())
        .map(|d| format!(", newest write {}s ago", d.as_secs()))
        .unwrap_or_default();
    Check::pass(
        "wal",
        format!("{base}: {segments} segment(s), {consumers} consumer position(s){age}"),
    )
}

/// Reachability probe: any HTTP response proves DNS + TCP + TLS + HTTP work;
/// the status code itself is reported but not judged (bases often 404).
async fn check_http(name: &'static str, url: &str) -> Check {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(e) => return Check::fail(name, format!("http client build failed: {e}")),
    };
    let started = Instant::now();
    match client.get(url).send().await {
        Ok(resp) => Check::pass(
            name,
            format!(
                "{url} reachable (HTTP {}, {} ms)",
                resp.status().as_u16(),
                started.elapsed().as_millis()
            ),
        ),
        Err(e) => Check::fail(name, format!("{url} unreachable: {e}")),
    }
}

async fn check_websocket(ws_url: &str) -> Check {
    match tokio::time::timeout(Duration::from_secs(10), pb_feed::probe_ws(ws_url)).await {
        Ok(Ok(latency)) => Check::pass(
            "websocket",
            format!("{ws_url} handshake ok ({} ms)", latency.as_millis()),
        ),
        Ok(Err(e)) => Check::fail("websocket", format!("{ws_url} handshake failed: {e}")),
        Err(_) => Check::fail("websocket", format!("{ws_url} handshake timed out")),
    }
}

/// A bindable port is available; a bound one usually means another poly-book
/// instance is already running — worth knowing, not necessarily wrong.
async fn check_port(name: &'static str, addr: SocketAddr) -> Check {
    match tokio::net::TcpListener::bind(addr).await {
        Ok(_) => Check::pass(name, format!("{addr} available")),
        Err(_) => Check::warn(name, format!("{addr} in use (another instance running?)")),
    }
}
