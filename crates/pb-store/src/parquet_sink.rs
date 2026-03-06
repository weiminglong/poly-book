use std::sync::Arc;
use std::time::Duration;

use object_store::ObjectStore;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use pb_types::PersistedRecord;

use crate::error::StoreError;
use crate::writer::ParquetRecordWriter;

const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(300);

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
        let mut buffer: Vec<PersistedRecord> = Vec::new();
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

    async fn flush(&self, buffer: &mut Vec<PersistedRecord>) -> Result<(), StoreError> {
        ParquetRecordWriter::new(self.store.clone(), self.base_path.clone())
            .write_batch(buffer.as_slice())
            .await?;
        buffer.clear();
        Ok(())
    }
}
