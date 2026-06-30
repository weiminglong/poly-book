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

    #[error("websocket connection stalled — no data within the read-idle timeout")]
    ConnectionStalled,

    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("{label} payload too large: {len} bytes exceeds {limit} byte limit")]
    PayloadTooLarge {
        label: &'static str,
        len: usize,
        limit: usize,
    },

    #[error("{label} array too large: {len} entries exceeds {limit} entry limit")]
    ArrayTooLarge {
        label: &'static str,
        len: usize,
        limit: usize,
    },

    #[error("book side too large for {label}: bids={bids}, asks={asks}, limit per side={limit}")]
    BookTooLarge {
        label: &'static str,
        bids: usize,
        asks: usize,
        limit: usize,
    },
}

impl From<tokio_tungstenite::tungstenite::Error> for FeedError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Ws(Box::new(e))
    }
}
