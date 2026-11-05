//! Typed runtime configuration.
//!
//! Loaded in layers — `config/default.toml`, then `PB__<SECTION>__<KEY>`
//! environment overrides — and deserialized once into [`Settings`]. Every key
//! is a struct field, so a misspelled or removed key is a startup error that
//! names the offending field instead of a silently ignored setting, and every
//! consumer reads a typed field instead of re-parsing a string with its own
//! fallback default.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub feed: FeedSettings,
    pub storage: StorageSettings,
    pub metrics: MetricsSettings,
    pub wal: WalSettings,
    pub api: ApiSettings,
    pub logging: LoggingSettings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FeedSettings {
    pub ws_url: String,
    pub rest_url: String,
    pub gamma_url: String,
    pub ping_interval_secs: u64,
    pub reconnect_base_delay_ms: u64,
    pub reconnect_max_delay_ms: u64,
    pub rate_limit_requests: u32,
    pub rate_limit_window_secs: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageSettings {
    /// Local path, `file://` URL, or `s3://bucket/prefix`.
    pub parquet_base_path: String,
    pub parquet_flush_interval_secs: u64,
    pub checkpoints_enabled: bool,
    pub checkpoint_interval_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsSettings {
    pub listen_addr: SocketAddr,
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WalSettings {
    pub base_path: PathBuf,
    pub segment_size_mb: u64,
    pub max_segments: usize,
    pub max_consumer_lag_bytes: u64,
    pub position_commit_interval_ms: u64,
    pub flush_interval_ms: u64,
    pub sync_interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiSettings {
    pub listen_addr: SocketAddr,
    pub default_depth: usize,
    pub max_depth: usize,
    pub stale_after_secs: u64,
    pub http_request_timeout_secs: u64,
    /// Bearer token required on every data route when set. Empty means unset.
    pub auth_token: Option<String>,
    /// Built `web/dist` to serve as the SPA fallback. Empty means API only.
    pub static_assets_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingSettings {
    pub level: String,
    /// `"json"` for structured output; anything else is the text formatter.
    pub format: String,
}

impl Default for FeedSettings {
    fn default() -> Self {
        let rest = pb_feed::RestConfig::default();
        Self {
            ws_url: pb_feed::WsConfig::default().ws_url,
            rest_url: rest.clob_base_url,
            gamma_url: rest.gamma_base_url,
            ping_interval_secs: 10,
            reconnect_base_delay_ms: 100,
            reconnect_max_delay_ms: 30_000,
            rate_limit_requests: 1_500,
            rate_limit_window_secs: 10,
        }
    }
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            parquet_base_path: "./data".to_string(),
            parquet_flush_interval_secs: 300,
            checkpoints_enabled: true,
            checkpoint_interval_secs: 60,
        }
    }
}

impl Default for MetricsSettings {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:9090".parse().expect("valid default addr"),
            endpoint: "/metrics".to_string(),
        }
    }
}

impl Default for WalSettings {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("./data/wal"),
            segment_size_mb: 64,
            max_segments: 16,
            max_consumer_lag_bytes: 256 * 1024 * 1024,
            position_commit_interval_ms: 1_000,
            flush_interval_ms: 20,
            sync_interval_ms: 200,
        }
    }
}

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:3000".parse().expect("valid default addr"),
            default_depth: 20,
            max_depth: 200,
            stale_after_secs: 15,
            http_request_timeout_secs: pb_api::DEFAULT_HTTP_REQUEST_TIMEOUT_SECS,
            auth_token: None,
            static_assets_dir: None,
        }
    }
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "pretty".to_string(),
        }
    }
}

impl Settings {
    /// Load `path` (required unless it is the default location and absent) and
    /// apply `PB__` environment overrides, then clamp pathological values.
    pub fn load(path: &str, required: bool) -> Result<Self> {
        let raw = config::Config::builder()
            .add_source(config::File::with_name(path).required(required))
            .add_source(config::Environment::with_prefix("PB").separator("__"))
            .build()
            .with_context(|| format!("failed to load config from {path}"))?;
        let mut settings: Settings = raw
            .try_deserialize()
            .with_context(|| format!("invalid config in {path} (or a PB__ override)"))?;
        settings.normalize();
        Ok(settings)
    }

    /// Clamp values that would busy-loop, allocate nothing, or disable a check
    /// (a 0 interval, a 0 segment size, a depth of 0) to their minimum, with a
    /// warning. Negative values are already rejected by the unsigned types.
    pub fn normalize(&mut self) {
        fn floor<T: Ord + Copy + std::fmt::Display>(key: &str, value: &mut T, min: T) {
            if *value < min {
                tracing::warn!(key, value = %*value, min = %min, "config value below minimum; clamping");
                *value = min;
            }
        }
        floor(
            "feed.ping_interval_secs",
            &mut self.feed.ping_interval_secs,
            1,
        );
        floor(
            "feed.reconnect_base_delay_ms",
            &mut self.feed.reconnect_base_delay_ms,
            1,
        );
        floor(
            "feed.reconnect_max_delay_ms",
            &mut self.feed.reconnect_max_delay_ms,
            1,
        );
        floor(
            "feed.rate_limit_requests",
            &mut self.feed.rate_limit_requests,
            1,
        );
        floor(
            "feed.rate_limit_window_secs",
            &mut self.feed.rate_limit_window_secs,
            1,
        );
        floor(
            "storage.parquet_flush_interval_secs",
            &mut self.storage.parquet_flush_interval_secs,
            1,
        );
        floor(
            "storage.checkpoint_interval_secs",
            &mut self.storage.checkpoint_interval_secs,
            1,
        );
        floor("wal.segment_size_mb", &mut self.wal.segment_size_mb, 1);
        floor("wal.max_segments", &mut self.wal.max_segments, 1);
        floor(
            "wal.max_consumer_lag_bytes",
            &mut self.wal.max_consumer_lag_bytes,
            1,
        );
        floor(
            "wal.position_commit_interval_ms",
            &mut self.wal.position_commit_interval_ms,
            1,
        );
        floor("wal.flush_interval_ms", &mut self.wal.flush_interval_ms, 1);
        floor("wal.sync_interval_ms", &mut self.wal.sync_interval_ms, 1);
        floor("api.max_depth", &mut self.api.max_depth, 1);
        floor("api.default_depth", &mut self.api.default_depth, 1);
        floor("api.stale_after_secs", &mut self.api.stale_after_secs, 1);
        floor(
            "api.http_request_timeout_secs",
            &mut self.api.http_request_timeout_secs,
            1,
        );
        // A default depth above the max would request more levels than any
        // query is allowed to return.
        if self.api.default_depth > self.api.max_depth {
            self.api.default_depth = self.api.max_depth;
        }
    }

    pub fn wal_config(&self) -> pb_wal::WalConfig {
        pb_wal::WalConfig {
            base_path: self.wal.base_path.clone(),
            segment_size: self.wal.segment_size_mb * 1024 * 1024,
            max_segments: self.wal.max_segments,
            max_consumer_lag_bytes: self.wal.max_consumer_lag_bytes,
            position_commit_interval_ms: self.wal.position_commit_interval_ms,
            flush_interval_ms: self.wal.flush_interval_ms,
            sync_interval_ms: self.wal.sync_interval_ms,
        }
    }

    pub fn ws_config(&self) -> pb_feed::WsConfig {
        pb_feed::WsConfig {
            ws_url: self.feed.ws_url.clone(),
            ping_interval_secs: self.feed.ping_interval_secs,
            reconnect_base_delay_ms: self.feed.reconnect_base_delay_ms,
            reconnect_max_delay_ms: self.feed.reconnect_max_delay_ms,
        }
    }

    pub fn rest_config(&self) -> pb_feed::RestConfig {
        pb_feed::RestConfig {
            clob_base_url: self.feed.rest_url.clone(),
            gamma_base_url: self.feed.gamma_url.clone(),
        }
    }

    pub fn rate_limiter(&self) -> pb_feed::RateLimiter {
        pb_feed::RateLimiter::with_window(
            self.feed.rate_limit_requests,
            self.feed.rate_limit_window_secs,
        )
    }

    /// A REST client configured from `[feed]`.
    pub fn rest_client(&self) -> Result<pb_feed::RestClient> {
        Ok(pb_feed::RestClient::new(self.rate_limiter())?.with_config(self.rest_config()))
    }

    pub fn checkpoint_config(
        &self,
    ) -> crate::commands::checkpoint_producer::CheckpointProducerConfig {
        crate::commands::checkpoint_producer::CheckpointProducerConfig {
            rest_url: self.feed.rest_url.clone(),
            interval: Duration::from_secs(self.storage.checkpoint_interval_secs),
            rate_limit_pause: Duration::from_millis(100),
        }
    }

    /// The API bearer token, with blank values treated as unset.
    pub fn api_auth_token(&self) -> Option<String> {
        self.api
            .auth_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(ToOwned::to_owned)
    }

    /// The bundled-SPA directory, with blank values treated as unset.
    pub fn static_assets_dir(&self) -> Option<String> {
        self.api
            .static_assets_dir
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(ToOwned::to_owned)
    }

    /// The `pb_api::ApiConfig` for the HTTP server, with the historical data
    /// root overridable (the demo serves its own capture directory).
    pub fn api_config(&self, parquet_base_path: String) -> pb_api::ApiConfig {
        pb_api::ApiConfig {
            parquet_base_path,
            default_depth: self.api.default_depth,
            max_depth: self.api.max_depth,
            stale_after_secs: self.api.stale_after_secs,
            http_request_timeout_secs: self.api.http_request_timeout_secs,
            auth_token: self.api_auth_token(),
            static_assets_dir: self.static_assets_dir(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_toml(text: &str) -> Result<Settings> {
        let raw = config::Config::builder()
            .add_source(config::File::from_str(text, config::FileFormat::Toml))
            .build()?;
        let mut s: Settings = raw.try_deserialize()?;
        s.normalize();
        Ok(s)
    }

    #[test]
    fn shipped_default_toml_matches_struct_defaults() {
        let shipped = from_toml(include_str!("../../../config/default.toml")).unwrap();
        let defaults = Settings::default();
        assert_eq!(format!("{shipped:?}"), format!("{defaults:?}"));
    }

    #[test]
    fn unknown_key_is_an_error() {
        let err = from_toml("[wal]\nsegment_size_md = 64\n").unwrap_err();
        assert!(err.to_string().contains("segment_size_md"), "{err}");
    }

    #[test]
    fn negative_value_is_an_error_and_zero_is_clamped() {
        assert!(from_toml("[wal]\nmax_consumer_lag_bytes = -1\n").is_err());
        let s = from_toml("[wal]\nsegment_size_mb = 0\nflush_interval_ms = 0\n").unwrap();
        assert_eq!(s.wal.segment_size_mb, 1);
        assert_eq!(s.wal.flush_interval_ms, 1);
    }

    #[test]
    fn default_depth_is_clamped_to_max_depth() {
        let s = from_toml("[api]\ndefault_depth = 500\nmax_depth = 100\n").unwrap();
        assert_eq!(s.api.default_depth, 100);
    }

    #[test]
    fn blank_auth_token_and_assets_dir_are_unset() {
        let s = from_toml("[api]\nauth_token = \"  \"\nstatic_assets_dir = \"\"\n").unwrap();
        assert_eq!(s.api_auth_token(), None);
        assert_eq!(s.static_assets_dir(), None);
        let s = from_toml("[api]\nauth_token = \" secret \"\n").unwrap();
        assert_eq!(s.api_auth_token().as_deref(), Some("secret"));
    }

    #[test]
    fn wal_config_scales_segment_size() {
        let s = from_toml("[wal]\nbase_path = \"/tmp/wal\"\nsegment_size_mb = 32\n").unwrap();
        let cfg = s.wal_config();
        assert_eq!(cfg.base_path, PathBuf::from("/tmp/wal"));
        assert_eq!(cfg.segment_size, 32 * 1024 * 1024);
        assert_eq!(cfg.max_segments, 16);
    }
}
