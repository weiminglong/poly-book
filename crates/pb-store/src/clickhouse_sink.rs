use std::time::Duration;

use clickhouse::Client;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use pb_types::event::PersistedRecord;

use crate::error::StoreError;
use crate::writer::ClickHouseRecordWriter;

const DEFAULT_BATCH_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_BATCH_SIZE: usize = 10_000;
/// Bounded flush retries before giving up, so a single transient insert failure
/// no longer tears down the whole ingest pipeline (audit findings A.5/A.12/A.26).
const MAX_FLUSH_RETRIES: u32 = 5;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(200);

pub struct ClickHouseSink {
    rx: mpsc::Receiver<PersistedRecord>,
    client: Client,
    batch_size: usize,
    batch_interval: Duration,
}

impl ClickHouseSink {
    pub fn new(rx: mpsc::Receiver<PersistedRecord>, client: Client) -> Self {
        Self {
            rx,
            client,
            batch_size: DEFAULT_BATCH_SIZE,
            batch_interval: DEFAULT_BATCH_INTERVAL,
        }
    }

    /// Override the batch size and flush interval from config (audit finding
    /// A.54 — these keys were previously documented but ignored). Zero values
    /// fall back to the defaults.
    pub fn with_batch_config(mut self, batch_size: usize, batch_interval: Duration) -> Self {
        if batch_size > 0 {
            self.batch_size = batch_size;
        }
        if !batch_interval.is_zero() {
            self.batch_interval = batch_interval;
        }
        self
    }

    pub async fn ensure_table(&self) -> Result<(), StoreError> {
        self.ensure_tables().await
    }

    pub async fn ensure_tables(&self) -> Result<(), StoreError> {
        ClickHouseRecordWriter::new(self.client.clone())
            .ensure_tables()
            .await
    }

    pub async fn run(self) -> Result<(), StoreError> {
        self.run_with_token(CancellationToken::new()).await
    }

    pub async fn run_with_token(mut self, token: CancellationToken) -> Result<(), StoreError> {
        let mut buffer: Vec<PersistedRecord> = Vec::with_capacity(self.batch_size);
        let mut interval = tokio::time::interval(self.batch_interval);
        interval.tick().await;

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    if !buffer.is_empty() {
                        tracing::info!(buffered = buffer.len(), "ClickHouseSink flushing on shutdown");
                        self.flush(&mut buffer).await?;
                    }
                    tracing::info!("ClickHouseSink graceful shutdown complete");
                    return Ok(());
                }
                record = self.rx.recv() => {
                    match record {
                        Some(record) => {
                            buffer.push(record);
                            if buffer.len() >= self.batch_size {
                                self.flush(&mut buffer).await?;
                            }
                        }
                        None => {
                            if !buffer.is_empty() {
                                self.flush(&mut buffer).await?;
                            }
                            tracing::info!("ClickHouseSink channel closed, shutting down");
                            return Ok(());
                        }
                    }
                }
                _ = interval.tick() => {
                    if !buffer.is_empty() {
                        self.flush(&mut buffer).await?;
                    }
                }
            }
        }
    }

    /// Flush the buffer with bounded exponential-backoff retries. The buffer is
    /// retained (not cleared) across retries and on terminal failure, so a
    /// transient ClickHouse error does not drop the batch or instantly kill the
    /// pipeline — only after `MAX_FLUSH_RETRIES` consecutive failures does this
    /// return an error (audit findings A.5/A.12/A.26).
    async fn flush(&self, buffer: &mut Vec<PersistedRecord>) -> Result<(), StoreError> {
        let mut attempt = 0u32;
        loop {
            match ClickHouseRecordWriter::new(self.client.clone())
                .write_batch(buffer.as_slice())
                .await
            {
                Ok(()) => {
                    buffer.clear();
                    return Ok(());
                }
                Err(e) => {
                    attempt += 1;
                    pb_metrics::record_sink_flush_failure("clickhouse");
                    if attempt > MAX_FLUSH_RETRIES {
                        tracing::error!(
                            error = %e,
                            retries = MAX_FLUSH_RETRIES,
                            buffered = buffer.len(),
                            "ClickHouse flush failed after retries; buffer retained"
                        );
                        return Err(e);
                    }
                    let backoff = RETRY_BASE_DELAY * 2u32.pow(attempt - 1);
                    tracing::warn!(
                        error = %e,
                        attempt,
                        backoff_ms = backoff.as_millis() as u64,
                        "ClickHouse flush failed; retrying (buffer retained)"
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
}
