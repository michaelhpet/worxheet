//! Provider presets and keychain-backed API key storage.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub struct PresetDef {
    pub name: &'static str,
    pub base_url: &'static str,
    pub default_model: &'static str,
}

/// Well-known OpenAI-compatible endpoints (`custom` covers anything else).
pub const PRESETS: &[PresetDef] = &[
    PresetDef {
        name: "openai",
        base_url: "https://api.openai.com/v1",
        default_model: "gpt-4o-mini",
    },
    PresetDef {
        name: "gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        default_model: "gemini-2.0-flash",
    },
    PresetDef {
        name: "ollama",
        base_url: "https://ollama.com/v1",
        default_model: "llama3.1",
    },
    PresetDef {
        name: "lmstudio",
        base_url: "http://localhost:1234/v1",
        default_model: "",
    },
];

const KEYRING_SERVICE: &str = "worxheet";

/// The API key lives in the OS keychain, joined at call time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerPresetConfig {
    pub base_url: String,
    pub model: String,
    pub concurrency: usize,
    /// Thinking models answer directly instead of burning tokens on reasoning.
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderSettings {
    pub active: String,
    pub presets: HashMap<String, PerPresetConfig>,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        let mut presets = HashMap::new();
        for preset in PRESETS {
            presets.insert(
                preset.name.to_string(),
                PerPresetConfig {
                    base_url: preset.base_url.to_string(),
                    model: preset.default_model.to_string(),
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
    pub fn active_config(&self) -> PerPresetConfig {
        self.presets.get(&self.active).cloned().unwrap_or_default()
    }
}

fn preset_def(preset: &str) -> Option<&'static PresetDef> {
    PRESETS.iter().find(|def| def.name == preset)
}

pub fn base_url_for_preset(preset: &str) -> Option<&'static str> {
    preset_def(preset).map(|def| def.base_url)
}

/// Empty stored keys count as absent.
pub fn active_key(preset: &str) -> Result<Option<String>, String> {
    Ok(load_api_key(preset)?.filter(|key| !key.is_empty()))
}

/// Per-preset slot so keys can never cross providers.
fn keyring_account(preset: &str) -> String {
    format!("provider-api-key:{preset}")
}

fn keyring_entry(preset: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &keyring_account(preset))
        .map_err(|e| format!("Keychain unavailable: {e}"))
}

/// An empty string deletes the entry.
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
            assert!(
                config.disable_thinking,
                "preset {preset} should default thinking disabled"
            );
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
