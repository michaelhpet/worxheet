//! Provider configuration, presets, and keychain-backed API key storage.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Well-known OpenAI-compatible endpoints. Any other deployment can be
/// reached through the `custom` preset with a user-supplied base URL.
pub const PRESETS: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com/v1"),
    (
        "gemini",
        "https://generativelanguage.googleapis.com/v1beta/openai",
    ),
    ("ollama", "https://ollama.com/v1"),
    ("lmstudio", "http://localhost:1234/v1"),
];

pub const DEFAULT_MODEL_BY_PRESET: &[(&str, &str)] = &[
    ("openai", "gpt-4o-mini"),
    ("gemini", "gemini-2.0-flash"),
    ("ollama", "llama3.1"),
    ("lmstudio", ""),
];

const KEYRING_SERVICE: &str = "worxheet";

/// Per-provider generation settings. The API key deliberately lives outside
/// this struct: it is stored in the OS keychain and only joined at call time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerPresetConfig {
    pub base_url: String,
    pub model: String,
    /// Parallel in-flight requests during bulk generation.
    pub concurrency: usize,
    /// Send `reasoning_effort: "none"` so thinking models answer directly
    /// instead of burning tokens on reasoning (and burying the answer in a
    /// `reasoning` field some OpenAI-compatible deployments return empty).
    /// Defaults to on: fast, direct answers are the norm; users opt into
    /// thinking where a model benefits from it.
    pub disable_thinking: bool,
}

impl Default for PerPresetConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            model: String::new(),
            concurrency: 8,
            disable_thinking: true,
        }
    }
}

impl PerPresetConfig {
    pub fn is_configured(&self) -> bool {
        !self.base_url.is_empty() && !self.model.is_empty()
    }
}

/// Full provider state: which preset is active plus the saved config for
/// each configured preset. Switching presets loads that preset's saved
/// settings; changes are persisted per-preset so the user's per-provider
/// configuration survives across switches.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderSettings {
    pub active: String,
    pub presets: HashMap<String, PerPresetConfig>,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        let mut presets = HashMap::new();
        for &(name, url) in PRESETS {
            presets.insert(
                name.to_string(),
                PerPresetConfig {
                    base_url: url.to_string(),
                    model: default_model_for_preset(name).to_string(),
                    ..PerPresetConfig::default()
                },
            );
        }
        Self {
            active: "openai".to_string(),
            presets,
        }
    }
}

impl ProviderSettings {
    /// The active preset's config, falling back to defaults if the preset
    /// was never explicitly configured.
    pub fn active_config(&self) -> PerPresetConfig {
        self.presets
            .get(&self.active)
            .cloned()
            .unwrap_or_default()
    }
}

pub fn base_url_for_preset(preset: &str) -> Option<&'static str> {
    PRESETS
        .iter()
        .find(|(name, _)| *name == preset)
        .map(|(_, url)| *url)
}

pub fn default_model_for_preset(preset: &str) -> &'static str {
    DEFAULT_MODEL_BY_PRESET
        .iter()
        .find(|(name, _)| *name == preset)
        .map(|(_, model)| *model)
        .unwrap_or("")
}

/// Keychain account for a given provider preset. Each provider owns its own
/// slot so a key can never be sent to a different provider.
fn keyring_account(preset: &str) -> String {
    format!("provider-api-key:{preset}")
}

fn keyring_entry(preset: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &keyring_account(preset))
        .map_err(|e| format!("Keychain unavailable: {e}"))
}

/// Persist a provider's API key in the OS keychain. An empty string deletes
/// that provider's entry.
pub fn store_api_key(preset: &str, key: &str) -> Result<(), String> {
    let entry = keyring_entry(preset)?;
    if key.is_empty() {
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Failed to remove stored key: {e}")),
        }
    } else {
        entry
            .set_password(key)
            .map_err(|e| format!("Failed to store key in keychain: {e}"))
    }
}

/// Load the stored API key for a provider, if any.
pub fn load_api_key(preset: &str) -> Result<Option<String>, String> {
    match keyring_entry(preset)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to read key from keychain: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_disables_thinking() {
        let settings = ProviderSettings::default();
        assert!(settings.active_config().disable_thinking);
    }

    #[test]
    fn test_presets_keep_thinking_disabled() {
        let settings = ProviderSettings::default();
        for preset in ["openai", "gemini", "ollama", "lmstudio"] {
            let config = settings.presets.get(preset).unwrap();
            assert!(config.disable_thinking, "preset {preset} should default thinking disabled");
        }
    }

    #[test]
    fn test_active_config_falls_back_for_unknown_preset() {
        let settings = ProviderSettings {
            active: "nonexistent".to_string(),
            ..ProviderSettings::default()
        };
        let config = settings.active_config();
        assert!(config.base_url.is_empty());
        assert!(config.model.is_empty());
        assert!(!config.is_configured());
    }
}