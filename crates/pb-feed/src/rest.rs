use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use tracing::debug;

use crate::error::FeedError;
use crate::rate_limiter::RateLimiter;
use pb_types::wire::{ClobMarketInfo, GammaEvent, RestBookResponse};

/// TCP connect timeout for REST calls.
const REST_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Overall per-request timeout. Without this a hung venue request stalls
/// discovery/backfill (and `auto-ingest` market rotation) indefinitely.
const REST_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum REST response body parsed as JSON.
pub const MAX_REST_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Maximum Gamma events accepted in a single discovery page.
pub const MAX_GAMMA_EVENTS_PER_RESPONSE: usize = 1000;
/// Maximum CLOB book levels accepted per side from REST.
pub const MAX_REST_BOOK_LEVELS_PER_SIDE: usize = 10_000;
/// Maximum outcome tokens accepted from one CLOB market metadata response.
pub const MAX_CLOB_MARKET_TOKENS: usize = 512;

#[derive(Debug, Clone)]
pub struct RestConfig {
    pub clob_base_url: String,
    pub gamma_base_url: String,
}

impl Default for RestConfig {
    fn default() -> Self {
        Self {
            clob_base_url: "https://clob.polymarket.com".to_string(),
            gamma_base_url: "https://gamma-api.polymarket.com".to_string(),
        }
    }
}

pub struct RestClient {
    client: Client,
    rate_limiter: RateLimiter,
    config: RestConfig,
}

impl RestClient {
    pub fn new(rate_limiter: RateLimiter) -> Result<Self, FeedError> {
        // Propagate a build failure rather than falling back to Client::new(),
        // which would silently drop the connect/request timeouts and allow an
        // upstream REST call to hang indefinitely — unacceptable for an HFT feed
        //. build() only fails if the TLS backend can't initialize.
        let client = Client::builder()
            .connect_timeout(REST_CONNECT_TIMEOUT)
            .timeout(REST_REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            rate_limiter,
            config: RestConfig::default(),
        })
    }

    pub fn with_config(mut self, config: RestConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn fetch_book(&self, token_id: &str) -> Result<RestBookResponse, FeedError> {
        self.rate_limiter.acquire().await;
        pb_metrics::record_rest_request();
        // Build the URL via parse_with_params, which percent-encodes the value, so
        // a token_id containing `&`/`?`/`#` cannot inject extra query params; log
        // the id as a structured field rather than interpolated into the URL
        // (defense-in-depth).
        let url = reqwest::Url::parse_with_params(
            &format!("{}/book", self.config.clob_base_url),
            &[("token_id", token_id)],
        )?;
        debug!(token_id, "fetching book");
        let resp = self.client.get(url).send().await?;
        let resp = classify_response(resp)?;
        let book: RestBookResponse = read_json_limited(resp, "clob book").await?;
        if book.bids.len() > MAX_REST_BOOK_LEVELS_PER_SIDE
            || book.asks.len() > MAX_REST_BOOK_LEVELS_PER_SIDE
        {
            return Err(FeedError::BookTooLarge {
                label: "clob book",
                bids: book.bids.len(),
                asks: book.asks.len(),
                limit: MAX_REST_BOOK_LEVELS_PER_SIDE,
            });
        }
        Ok(book)
    }

    pub async fn discover_markets(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<GammaEvent>, FeedError> {
        self.rate_limiter.acquire().await;
        pb_metrics::record_rest_request();
        let request_limit = limit.min(MAX_GAMMA_EVENTS_PER_RESPONSE as u64);
        let url = format!(
            "{}/events?active=true&closed=false&tag=crypto&offset={offset}&limit={request_limit}",
            self.config.gamma_base_url
        );
        debug!(url, "discovering markets");
        let resp = self.client.get(&url).send().await?;
        let resp = classify_response(resp)?;
        let events: Vec<GammaEvent> = read_json_limited(resp, "gamma events").await?;
        if events.len() > MAX_GAMMA_EVENTS_PER_RESPONSE {
            return Err(FeedError::ArrayTooLarge {
                label: "gamma events",
                len: events.len(),
                limit: MAX_GAMMA_EVENTS_PER_RESPONSE,
            });
        }
        Ok(events)
    }

    /// Fetch a single event by exact slug.
    pub async fn discover_by_slug(&self, slug: &str) -> Result<Vec<GammaEvent>, FeedError> {
        self.rate_limiter.acquire().await;
        pb_metrics::record_rest_request();
        // parse_with_params percent-encodes the slug so it cannot inject extra
        // query params (defense-in-depth).
        let url = reqwest::Url::parse_with_params(
            &format!("{}/events", self.config.gamma_base_url),
            &[("slug", slug)],
        )?;
        debug!(slug, "discovering by slug");
        let resp = self.client.get(url).send().await?;
        let resp = classify_response(resp)?;
        let events: Vec<GammaEvent> = read_json_limited(resp, "gamma events").await?;
        if events.len() > MAX_GAMMA_EVENTS_PER_RESPONSE {
            return Err(FeedError::ArrayTooLarge {
                label: "gamma events",
                len: events.len(),
                limit: MAX_GAMMA_EVENTS_PER_RESPONSE,
            });
        }
        Ok(events)
    }

    /// Fetch V2 CLOB market metadata: min tick, min order size, fee schedule,
    /// token outcomes, and protocol flags. Available on CLOB V2 only.
    pub async fn get_clob_market_info(
        &self,
        condition_id: &str,
    ) -> Result<ClobMarketInfo, FeedError> {
        self.rate_limiter.acquire().await;
        pb_metrics::record_rest_request();
        let url = format!("{}/clob-markets/{condition_id}", self.config.clob_base_url);
        debug!(url, "fetching clob market info");
        let resp = self.client.get(&url).send().await?;
        let resp = classify_response(resp)?;
        let info: ClobMarketInfo = read_json_limited(resp, "clob market").await?;
        if info.t.len() > MAX_CLOB_MARKET_TOKENS {
            return Err(FeedError::ArrayTooLarge {
                label: "clob market tokens",
                len: info.t.len(),
                limit: MAX_CLOB_MARKET_TOKENS,
            });
        }
        Ok(info)
    }
}

async fn read_json_limited<T: DeserializeOwned>(
    mut resp: reqwest::Response,
    label: &'static str,
) -> Result<T, FeedError> {
    if let Some(len) = resp.content_length() {
        if len > MAX_REST_BODY_BYTES as u64 {
            return Err(FeedError::PayloadTooLarge {
                label,
                len: len.min(usize::MAX as u64) as usize,
                limit: MAX_REST_BODY_BYTES,
            });
        }
    }

    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let next_len = body.len().saturating_add(chunk.len());
        if next_len > MAX_REST_BODY_BYTES {
            return Err(FeedError::PayloadTooLarge {
                label,
                len: next_len,
                limit: MAX_REST_BODY_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    if body.len() > MAX_REST_BODY_BYTES {
        return Err(FeedError::PayloadTooLarge {
            label,
            len: body.len(),
            limit: MAX_REST_BODY_BYTES,
        });
    }
    serde_json::from_slice(&body).map_err(FeedError::Deserialize)
}

/// Classify an HTTP status code into the appropriate FeedError.
/// Returns Ok(()) for success, Err for errors.
pub(crate) fn classify_status(status: StatusCode) -> Result<(), FeedError> {
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(FeedError::RateLimited);
    }
    Err(FeedError::HttpStatus {
        status: status.as_u16(),
    })
}

/// Classify HTTP response status codes into appropriate errors.
/// Maps 429 -> FeedError::RateLimited, other non-2xx -> FeedError::Rest via error_for_status.
fn classify_response(resp: reqwest::Response) -> Result<reqwest::Response, FeedError> {
    let status = resp.status();
    classify_status(status)?;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn test_classify_status_success() {
        assert!(classify_status(StatusCode::OK).is_ok());
        assert!(classify_status(StatusCode::CREATED).is_ok());
        assert!(classify_status(StatusCode::NO_CONTENT).is_ok());
    }

    #[test]
    fn test_classify_status_rate_limited() {
        let err = classify_status(StatusCode::TOO_MANY_REQUESTS).unwrap_err();
        assert!(matches!(err, FeedError::RateLimited));
    }

    #[test]
    fn test_classify_status_server_error() {
        let err = classify_status(StatusCode::INTERNAL_SERVER_ERROR).unwrap_err();
        assert!(matches!(err, FeedError::HttpStatus { status: 500 }));
    }

    #[test]
    fn test_classify_status_client_error() {
        let err = classify_status(StatusCode::NOT_FOUND).unwrap_err();
        assert!(matches!(err, FeedError::HttpStatus { status: 404 }));
    }

    #[test]
    fn test_classify_status_bad_gateway() {
        let err = classify_status(StatusCode::BAD_GATEWAY).unwrap_err();
        assert!(matches!(err, FeedError::HttpStatus { status: 502 }));
    }

    #[tokio::test]
    async fn read_json_limited_rejects_oversized_content_length_before_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                MAX_REST_BODY_BYTES + 1
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let resp = reqwest::get(format!("http://{addr}/oversized"))
            .await
            .unwrap();
        let err = tokio::time::timeout(
            Duration::from_secs(1),
            read_json_limited::<serde_json::Value>(resp, "test payload"),
        )
        .await
        .expect("oversized Content-Length should be rejected before body read")
        .unwrap_err();

        server.abort();
        assert!(matches!(
            err,
            FeedError::PayloadTooLarge {
                label: "test payload",
                len,
                limit: MAX_REST_BODY_BYTES,
            } if len == MAX_REST_BODY_BYTES + 1
        ));
    }
}
