use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetricsError {
    #[error("failed to install metrics recorder: {0}")]
    RecorderInstall(String),

    #[error("failed to start metrics server: {0}")]
    ServerStart(String),
}
