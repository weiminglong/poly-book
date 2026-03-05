use thiserror::Error;

#[derive(Debug, Error)]
pub enum FeedError {
    #[error("websocket error: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("rest error: {0}")]
    Rest(#[from] reqwest::Error),

    #[error("deserialization error: {0}")]
    Deserialize(#[from] serde_json::Error),

    #[error("type conversion error: {0}")]
    Types(#[from] pb_types::TypesError),

    #[error("rate limited")]
    RateLimited,

    #[error("channel send error")]
    ChannelSend,

    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),
}
