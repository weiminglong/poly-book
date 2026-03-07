use thiserror::Error;

#[derive(Debug, Error)]
pub enum FeedError {
    #[error("websocket error: {0}")]
    Ws(Box<tokio_tungstenite::tungstenite::Error>),

    #[error("rest API error: {0}")]
    Rest(#[from] reqwest::Error),

    #[error("feed deserialization error: {0}")]
    Deserialize(#[from] serde_json::Error),

    #[error("feed type conversion error: {0}")]
    Types(#[from] pb_types::TypesError),

    #[error("rate limited — back-off before retrying")]
    RateLimited,

    #[error("HTTP {status}: unexpected status from upstream API")]
    HttpStatus { status: u16 },

    #[error("output channel closed — downstream consumer stopped")]
    ChannelSend,

    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("TLS handshake error: {0}")]
    Tls(#[from] native_tls::Error),
}

impl From<tokio_tungstenite::tungstenite::Error> for FeedError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Ws(Box::new(e))
    }
}
