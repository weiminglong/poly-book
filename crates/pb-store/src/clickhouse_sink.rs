use std::time::Duration;

use clickhouse::Client;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use pb_types::event::PersistedRecord;

use crate::error::StoreError;
use crate::writer::ClickHouseRecordWriter;

const DEFAULT_BATCH_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_BATCH_SIZE: usize = 10_000;

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

    async fn flush(&self, buffer: &mut Vec<PersistedRecord>) -> Result<(), StoreError> {
        ClickHouseRecordWriter::new(self.client.clone())
            .write_batch(buffer.as_slice())
            .await?;
        buffer.clear();
        Ok(())
    }
}
