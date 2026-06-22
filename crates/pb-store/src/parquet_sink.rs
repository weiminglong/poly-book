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
///.
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
                    // Drain records still queued in the channel before the final
                    // flush, so a graceful stop does not abandon records the
                    // upstream already sent but the sink had not yet received
                    //. Bounded so a stuck upstream cannot
                    // block shutdown forever.
                    let drain_complete =
                        self.drain_channel(&mut buffer, Duration::from_secs(10)).await;
                    if !buffer.is_empty() {
                        tracing::info!(buffered = buffer.len(), "ParquetSink flushing on shutdown");
                        self.flush(&mut buffer).await?;
                    }
                    if !drain_complete {
                        // The drain hit its deadline with records still queued: an
                        // incomplete shutdown that abandons data. Surface it as an
                        // error so the supervisor records the failure instead of a
                        // clean stop (mirrors ClickHouseSink).
                        // The abandoned records remain durable in the WAL and are
                        // recoverable via `reconcile`.
                        let remaining = self.rx.len();
                        tracing::error!(
                            remaining,
                            "ParquetSink drain timed out on shutdown; {remaining} records abandoned (recoverable from WAL via reconcile)"
                        );
                        return Err(StoreError::Other(format!(
                            "ParquetSink shutdown drain timed out with {remaining} records still queued"
                        )));
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

    /// Drain records still queued in the channel into `buffer` on shutdown, up to
    /// a deadline. The upstream drops its sender during a coordinated shutdown, so
    /// `recv()` returns `None` once the backlog is drained; the timeout guards
    /// against an upstream that never closes.
    ///
    /// Returns `true` if the channel drained to completion (`recv() → None`), or
    /// `false` if the deadline was hit with records potentially still queued — the
    /// caller surfaces that as an incomplete-shutdown error.
    async fn drain_channel(
        &mut self,
        buffer: &mut Vec<PersistedRecord>,
        deadline: Duration,
    ) -> bool {
        let drained = tokio::time::timeout(deadline, async {
            let mut count = 0usize;
            while let Some(record) = self.rx.recv().await {
                buffer.push(record);
                count += 1;
            }
            count
        })
        .await;
        match drained {
            Ok(count) => {
                if count > 0 {
                    tracing::info!(
                        drained = count,
                        "ParquetSink drained channel backlog on shutdown"
                    );
                }
                true
            }
            Err(_) => false,
        }
    }

    /// Flush the buffer with bounded exponential-backoff retries. The buffer is
    /// retained (not cleared) across retries and on terminal failure, so a
    /// transient object-store write error does not drop the batch or instantly
    /// kill the pipeline.
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
