use reqwest::{Client, StatusCode};
use tracing::debug;

use crate::error::FeedError;
use crate::rate_limiter::RateLimiter;
use pb_types::wire::{GammaEvent, RestBookResponse};

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
    pub fn new(rate_limiter: RateLimiter) -> Self {
        Self {
            client: Client::new(),
            rate_limiter,
            config: RestConfig::default(),
        }
    }

    pub fn with_config(mut self, config: RestConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn fetch_book(&self, token_id: &str) -> Result<RestBookResponse, FeedError> {
        self.rate_limiter.acquire().await;
        pb_metrics::record_rest_request();
        let url = format!("{}/book?token_id={token_id}", self.config.clob_base_url);
        debug!(url, "fetching book");
        let resp = self.client.get(&url).send().await?;
        let resp = classify_response(resp)?;
        let book = resp.json().await?;
        Ok(book)
    }

    pub async fn discover_markets(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<GammaEvent>, FeedError> {
        self.rate_limiter.acquire().await;
        pb_metrics::record_rest_request();
        let url = format!(
            "{}/events?active=true&closed=false&tag=crypto&offset={offset}&limit={limit}",
            self.config.gamma_base_url
        );
        debug!(url, "discovering markets");
        let resp = self.client.get(&url).send().await?;
        let resp = classify_response(resp)?;
        let events = resp.json().await?;
        Ok(events)
    }

    /// Fetch a single event by exact slug.
    pub async fn discover_by_slug(&self, slug: &str) -> Result<Vec<GammaEvent>, FeedError> {
        self.rate_limiter.acquire().await;
        pb_metrics::record_rest_request();
        let url = format!(
            "{}/events?slug={slug}",
            self.config.gamma_base_url
        );
        debug!(url, "discovering by slug");
        let resp = self.client.get(&url).send().await?;
        let resp = classify_response(resp)?;
        let events = resp.json().await?;
        Ok(events)
    }
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
    Err(FeedError::HttpStatus(status.as_u16()))
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
        assert!(matches!(err, FeedError::HttpStatus(500)));
    }

    #[test]
    fn test_classify_status_client_error() {
        let err = classify_status(StatusCode::NOT_FOUND).unwrap_err();
        assert!(matches!(err, FeedError::HttpStatus(404)));
    }

    #[test]
    fn test_classify_status_bad_gateway() {
        let err = classify_status(StatusCode::BAD_GATEWAY).unwrap_err();
        assert!(matches!(err, FeedError::HttpStatus(502)));
    }
}
