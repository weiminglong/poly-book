//! Startup validation of configuration keys.
//!
//! Config is loaded from `config/default.toml` plus `PB__`-prefixed environment
//! overrides and read ad-hoc via `settings.get_*` / `cfg_int_min`. A misspelled
//! key (e.g. `wal.segment_size_md`) was previously silently ignored — the default
//! was used and the operator's intent was lost with no feedback.
//!
//! This module warns (loudly, at startup) about any key present in the loaded
//! config that the application does not recognize, so a typo is visible. It warns
//! rather than hard-errors: a key accidentally omitted from the whitelist then
//! produces only a spurious warning instead of breaking startup of an always-on
//! ingest process. `KNOWN_CONFIG_KEYS` is kept in sync with the code + default
//! TOML by `config_keys_cover_default_toml` (a test that fails on drift).

/// Every fully-qualified `section.key` the application understands. This is the
/// union of keys read by the code and keys documented in `config/default.toml`
/// (including optional/commented ones like `api.auth_token` and the reserved
/// `storage.parquet_row_group_size`).
pub const KNOWN_CONFIG_KEYS: &[&str] = &[
    // [feed]
    "feed.ws_url",
    "feed.rest_url",
    "feed.gamma_url",
    "feed.ping_interval_secs",
    "feed.reconnect_base_delay_ms",
    "feed.reconnect_max_delay_ms",
    "feed.rate_limit_requests",
    "feed.rate_limit_window_secs",
    // [storage]
    "storage.parquet_base_path",
    "storage.parquet_flush_interval_secs",
    "storage.parquet_row_group_size",
    "storage.checkpoints_enabled",
    "storage.checkpoint_interval_secs",
    "storage.clickhouse_url",
    "storage.clickhouse_database",
    "storage.clickhouse_batch_interval_secs",
    "storage.clickhouse_batch_size",
    // [metrics]
    "metrics.listen_addr",
    "metrics.endpoint",
    // [wal]
    "wal.base_path",
    "wal.segment_size_mb",
    "wal.max_segments",
    "wal.max_consumer_lag_bytes",
    "wal.position_commit_interval_ms",
    "wal.flush_interval_ms",
    "wal.sync_interval_ms",
    // [api]
    "api.listen_addr",
    "api.default_depth",
    "api.max_depth",
    "api.stale_after_secs",
    "api.historical_backend",
    "api.query_workbench_enabled",
    "api.query_max_rows",
    "api.query_timeout_secs",
    "api.http_request_timeout_secs",
    "api.auth_token",
    // [grpc]
    "grpc.enabled",
    "grpc.listen_addr",
    // [logging]
    "logging.level",
    "logging.format",
];

/// Warn about every key in the loaded config that is not in `KNOWN_CONFIG_KEYS`.
/// A no-op if the config cannot be projected to a JSON object (it always can in
/// practice). Call once at startup, after tracing is initialized.
pub fn warn_unknown_config_keys(settings: &config::Config) {
    let Ok(value) = settings.clone().try_deserialize::<serde_json::Value>() else {
        return;
    };
    for key in unknown_config_keys(&value, KNOWN_CONFIG_KEYS) {
        tracing::warn!(
            config_key = %key,
            "unknown config key — ignored (using the default); check for a typo or a removed/renamed setting"
        );
    }
}

/// Pure helper: return the `section.key` (or bare top-level) keys present in
/// `value` that are not in `known`, sorted for deterministic output.
fn unknown_config_keys(value: &serde_json::Value, known: &[&str]) -> Vec<String> {
    let mut unknown = Vec::new();
    if let Some(sections) = value.as_object() {
        for (section, section_value) in sections {
            match section_value.as_object() {
                Some(keys) => {
                    for key in keys.keys() {
                        let full = format!("{section}.{key}");
                        if !known.contains(&full.as_str()) {
                            unknown.push(full);
                        }
                    }
                }
                // A non-table at the top level is itself an unrecognized key
                // (the schema has only `[section] key = ...` entries).
                None => unknown.push(section.clone()),
            }
        }
    }
    unknown.sort();
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_unknown_section_key_and_passes_known() {
        let value = serde_json::json!({
            "wal": { "segment_size_mb": 64, "segment_size_md": 64 }, // typo'd second key
            "api": { "default_depth": 20 },
        });
        let unknown = unknown_config_keys(&value, KNOWN_CONFIG_KEYS);
        assert_eq!(unknown, vec!["wal.segment_size_md".to_string()]);
    }

    #[test]
    fn flags_unknown_section_and_bare_top_level_key() {
        let value = serde_json::json!({
            "feed": { "bogus": 1 },
            "stray_top_level": "x",
        });
        let unknown = unknown_config_keys(&value, KNOWN_CONFIG_KEYS);
        assert_eq!(
            unknown,
            vec!["feed.bogus".to_string(), "stray_top_level".to_string()]
        );
    }

    #[test]
    fn clean_config_yields_no_warnings() {
        let value = serde_json::json!({
            "wal": { "segment_size_mb": 64, "max_segments": 16 },
            "api": { "auth_token": "x", "http_request_timeout_secs": 30 },
            "logging": { "level": "info" },
        });
        assert!(unknown_config_keys(&value, KNOWN_CONFIG_KEYS).is_empty());
    }

    /// Every uncommented key in `config/default.toml` must be in
    /// `KNOWN_CONFIG_KEYS`, so the whitelist cannot drift away from the shipped
    /// defaults without failing CI.
    #[test]
    fn config_keys_cover_default_toml() {
        let toml_text = include_str!("../../../config/default.toml");
        let parsed: toml::Value = toml::from_str(toml_text).expect("default.toml parses");
        let table = parsed.as_table().expect("default.toml is a table");
        let mut missing = Vec::new();
        for (section, section_val) in table {
            if let Some(keys) = section_val.as_table() {
                for key in keys.keys() {
                    let full = format!("{section}.{key}");
                    if !KNOWN_CONFIG_KEYS.contains(&full.as_str()) {
                        missing.push(full);
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "config/default.toml keys missing from KNOWN_CONFIG_KEYS: {missing:?}"
        );
    }
}
