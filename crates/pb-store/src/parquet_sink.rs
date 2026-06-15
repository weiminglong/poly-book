use std::sync::Arc;
use std::time::Duration;

use object_store::ObjectStore;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use pb_types::PersistedRecord;

use crate::error::StoreError;
use crate::writer::ParquetRecordWriter;

const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(300);
/// Bounded flush retries before giving up, so a single transient object-store
/// write failure does not drop the buffered batch or kill the pipeline
/// (audit findings A.5/A.12/A.26).
const MAX_FLUSH_RETRIES: u32 = 5;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(200);

pub struct ParquetSink {
    rx: mpsc::Receiver<PersistedRecord>,
    store: Arc<dyn ObjectStore>,
    base_path: String,
    flush_interval: Duration,
}

impl ParquetSink {
    pub fn new(
        rx: mpsc::Receiver<PersistedRecord>,
        store: Arc<dyn ObjectStore>,
        base_path: String,
    ) -> Self {
        Self {
            rx,
            store,
            base_path,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
        }
    }

    pub fn with_flush_interval(mut self, interval: Duration) -> Self {
        self.flush_interval = interval;
        self
    }

    pub async fn run(self) -> Result<(), StoreError> {
        self.run_with_token(CancellationToken::new()).await
    }

    pub async fn run_with_token(mut self, token: CancellationToken) -> Result<(), StoreError> {
        let mut buffer: Vec<PersistedRecord> = Vec::with_capacity(4096);
        let mut interval = tokio::time::interval(self.flush_interval);
        interval.tick().await;

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    if !buffer.is_empty() {
                        tracing::info!(buffered = buffer.len(), "ParquetSink flushing on shutdown");
                        self.flush(&mut buffer).await?;
                    }
                    tracing::info!("ParquetSink graceful shutdown complete");
                    return Ok(());
                }
                record = self.rx.recv() => {
                    match record {
                        Some(record) => buffer.push(record),
                        None => {
                            if !buffer.is_empty() {
                                self.flush(&mut buffer).await?;
                            }
                            tracing::info!("ParquetSink channel closed, shutting down");
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
    /// transient object-store write error does not drop the batch or instantly
    /// kill the pipeline (audit findings A.5/A.12/A.26).
    async fn flush(&self, buffer: &mut Vec<PersistedRecord>) -> Result<(), StoreError> {
        let mut attempt = 0u32;
        loop {
            match ParquetRecordWriter::new(self.store.clone(), self.base_path.clone())
                .write_batch(buffer.as_slice())
                .await
            {
                Ok(()) => {
                    buffer.clear();
                    return Ok(());
                }
                Err(e) => {
                    attempt += 1;
                    pb_metrics::record_sink_flush_failure("parquet");
                    if attempt > MAX_FLUSH_RETRIES {
                        tracing::error!(
                            error = %e,
                            retries = MAX_FLUSH_RETRIES,
                            buffered = buffer.len(),
                            "Parquet flush failed after retries; buffer retained"
                        );
                        return Err(e);
                    }
                    let backoff = RETRY_BASE_DELAY * 2u32.pow(attempt - 1);
                    tracing::warn!(
                        error = %e,
                        attempt,
                        backoff_ms = backoff.as_millis() as u64,
                        "Parquet flush failed; retrying (buffer retained)"
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
}
