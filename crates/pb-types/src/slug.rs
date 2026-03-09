use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::AssetId;

/// Bidirectional mapping between human-readable slugs and full token IDs.
///
/// Populated during market discovery from Gamma API metadata.
/// Thread-safe for concurrent reads with infrequent writes.
#[derive(Debug, Clone)]
pub struct SlugRegistry {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    slug_to_token: HashMap<String, AssetId>,
    token_to_slug: HashMap<AssetId, String>,
    token_to_label: HashMap<AssetId, String>,
}

impl Default for SlugRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SlugRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner::default())),
        }
    }

    /// Register a single slug → token ID mapping.
    pub fn register(&self, slug: &str, asset_id: &AssetId) {
        let mut inner = self.inner.write().unwrap();
        inner
            .slug_to_token
            .insert(slug.to_string(), asset_id.clone());
        inner
            .token_to_slug
            .insert(asset_id.clone(), slug.to_string());
    }

    /// Register a market with one or two CLOB token IDs.
    ///
    /// For a single token, the slug is used as-is.
    /// For two tokens (YES/NO market), appends `-yes` and `-no` suffixes.
    pub fn register_market(&self, base_slug: &str, token_ids: &[String]) {
        match token_ids.len() {
            1 => {
                self.register(base_slug, &AssetId::new(token_ids[0].as_str()));
            }
            2 => {
                self.register(
                    &format!("{base_slug}-yes"),
                    &AssetId::new(token_ids[0].as_str()),
                );
                self.register(
                    &format!("{base_slug}-no"),
                    &AssetId::new(token_ids[1].as_str()),
                );
            }
            n if n > 2 => {
                for (i, token_id) in token_ids.iter().enumerate() {
                    self.register(
                        &format!("{base_slug}-{i}"),
                        &AssetId::new(token_id.as_str()),
                    );
                }
            }
            _ => {}
        }
    }

    /// Register a human-readable label for an asset.
    pub fn register_label(&self, asset_id: &AssetId, label: &str) {
        let mut inner = self.inner.write().unwrap();
        inner
            .token_to_label
            .insert(asset_id.clone(), label.to_string());
    }

    /// Resolve an input string to an `AssetId`.
    ///
    /// If the input is >40 characters and all digits, it's treated as a raw token ID.
    /// Otherwise, it's looked up as a slug.
    pub fn resolve(&self, input: &str) -> Option<AssetId> {
        if is_raw_token_id(input) {
            return Some(AssetId::new(input));
        }
        let inner = self.inner.read().unwrap();
        inner.slug_to_token.get(input).cloned()
    }

    /// Look up the slug for a given asset ID.
    pub fn slug_for(&self, asset_id: &AssetId) -> Option<String> {
        let inner = self.inner.read().unwrap();
        inner.token_to_slug.get(asset_id).cloned()
    }

    /// Look up the slug for a given asset ID string.
    pub fn slug_for_str(&self, asset_id: &str) -> Option<String> {
        self.slug_for(&AssetId::new(asset_id))
    }

    /// Look up the label for a given asset ID.
    pub fn label_for(&self, asset_id: &AssetId) -> Option<String> {
        let inner = self.inner.read().unwrap();
        inner.token_to_label.get(asset_id).cloned()
    }

    /// Look up the label for a given asset ID string.
    pub fn label_for_str(&self, asset_id: &str) -> Option<String> {
        self.label_for(&AssetId::new(asset_id))
    }
}

/// Returns true if the input looks like a raw Polymarket token ID
/// (more than 40 characters, all ASCII digits).
fn is_raw_token_id(input: &str) -> bool {
    input.len() > 40 && input.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_resolve_round_trip() {
        let registry = SlugRegistry::new();
        let asset = AssetId::new("1234567890".repeat(8));
        registry.register("btc-up-5m-1741500000", &asset);

        let resolved = registry.resolve("btc-up-5m-1741500000").unwrap();
        assert_eq!(resolved, asset);

        let slug = registry.slug_for(&asset).unwrap();
        assert_eq!(slug, "btc-up-5m-1741500000");
    }

    #[test]
    fn raw_token_id_passthrough() {
        let registry = SlugRegistry::new();
        let long_id = "21742633143463801764263866138596936600980228888098934498299596572218858267895";
        let resolved = registry.resolve(long_id).unwrap();
        assert_eq!(resolved.as_str(), long_id);
    }

    #[test]
    fn unknown_slug_returns_none() {
        let registry = SlugRegistry::new();
        assert!(registry.resolve("nonexistent-slug").is_none());
    }

    #[test]
    fn yes_no_suffix_for_two_token_market() {
        let registry = SlugRegistry::new();
        let token_yes = "11111111111111111111111111111111111111111111111111111111111111111111111";
        let token_no = "22222222222222222222222222222222222222222222222222222222222222222222222";
        registry.register_market(
            "btc-updown-5m-1741500000",
            &[token_yes.to_string(), token_no.to_string()],
        );

        let resolved_yes = registry.resolve("btc-updown-5m-1741500000-yes").unwrap();
        assert_eq!(resolved_yes.as_str(), token_yes);

        let resolved_no = registry.resolve("btc-updown-5m-1741500000-no").unwrap();
        assert_eq!(resolved_no.as_str(), token_no);
    }

    #[test]
    fn single_token_market_uses_base_slug() {
        let registry = SlugRegistry::new();
        let token = "33333333333333333333333333333333333333333333333333333333333333333333333";
        registry.register_market("some-market", &[token.to_string()]);

        let resolved = registry.resolve("some-market").unwrap();
        assert_eq!(resolved.as_str(), token);
    }

    #[test]
    fn short_numeric_string_is_not_raw_token_id() {
        let registry = SlugRegistry::new();
        // 20 digits — too short to be a raw token ID
        assert!(registry.resolve("12345678901234567890").is_none());
    }

    #[test]
    fn label_registration_and_lookup() {
        let registry = SlugRegistry::new();
        let asset = AssetId::new("tok1");
        registry.register_label(&asset, "BTC 5m UP 2026-03-09 14:00");

        assert_eq!(
            registry.label_for(&asset).unwrap(),
            "BTC 5m UP 2026-03-09 14:00"
        );
        assert_eq!(
            registry.label_for_str("tok1").unwrap(),
            "BTC 5m UP 2026-03-09 14:00"
        );
    }
}
