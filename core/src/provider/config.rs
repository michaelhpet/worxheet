//! Provider configuration, presets, and keychain-backed API key storage.

use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// Well-known OpenAI-compatible endpoints. Any other deployment can be
/// reached through the `custom` preset with a user-supplied base URL.
pub const PRESETS: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com/v1"),
    ("gemini", "https://generativelanguage.googleapis.com/v1beta/openai"),
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

/// Active generation settings. The API key deliberately lives outside this
/// struct: it is stored in the OS keychain and only joined at call time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// One of [`PRESETS`] names or `"custom"`.
    pub preset: String,
    pub base_url: String,
    pub model: String,
    /// Parallel in-flight requests during bulk generation.
    pub concurrency: usize,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            preset: String::from("openai"),
            base_url: base_url_for_preset("openai").unwrap_or_default().to_string(),
            model: default_model_for_preset("openai").to_string(),
            concurrency: 8,
        }
    }
}

impl ProviderConfig {
    #[allow(dead_code)] // used by frontend-driven flows and tests
    pub fn for_preset(preset: &str) -> Self {
        let mut config = Self::default();
        if let Some(base_url) = base_url_for_preset(preset) {
            config.preset = preset.to_string();
            config.base_url = base_url.to_string();
            config.model = default_model_for_preset(preset).to_string();
        } else {
            config.preset = String::from("custom");
            config.base_url = String::new();
            config.model = String::new();
        }
        config
    }

    pub fn is_configured(&self) -> bool {
        !self.base_url.is_empty() && !self.model.is_empty()
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

/// Process-wide holder of the active provider configuration.
pub struct ProviderState {
    config: RwLock<ProviderConfig>,
}

impl ProviderState {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config: RwLock::new(config),
        }
    }

    pub fn get(&self) -> ProviderConfig {
        self.config.read().unwrap().clone()
    }

    pub fn set(&self, config: ProviderConfig) {
        *self.config.write().unwrap() = config;
    }
}
