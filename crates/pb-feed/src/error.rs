use thiserror::Error;

#[derive(Debug, Error)]
pub enum FeedError {
    #[error("websocket error: {0}")]
    Ws(Box<tokio_tungstenite::tungstenite::Error>),

    #[error("rest error: {0}")]
    Rest(#[from] reqwest::Error),

    #[error("deserialization error: {0}")]
    Deserialize(#[from] serde_json::Error),

    #[error("type conversion error: {0}")]
    Types(#[from] pb_types::TypesError),

    #[error("rate limited")]
    RateLimited,

    #[error("HTTP status error: {0}")]
    HttpStatus(u16),

    #[error("channel send error")]
    ChannelSend,

    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),
}

impl From<tokio_tungstenite::tungstenite::Error> for FeedError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Ws(Box::new(e))
    }
}
