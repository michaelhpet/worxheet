//! App-wide settings persisted as JSON in the app data directory.
//!
//! One in-memory source of truth ([`SettingsState`]) backs all settings reads
//! and every write goes through the same write lock, then is flushed
//! atomically (temp file + rename) so a crash cannot corrupt the file.

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::provider::config::{PerPresetConfig, ProviderSettings};

/// Light/dark/system theme preference.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    Light,
    Dark,
    System,
}

impl Default for ThemePreference {
    fn default() -> Self {
        Self::System
    }
}

/// Appearance preferences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: ThemePreference,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::default(),
        }
    }
}

/// Generation tuning. Not consumed by the pipeline yet — persisted now so the
/// settings survive restarts ahead of wiring into generation requests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArtifactSettings {
    /// Creativity, 0..=2.
    pub temperature: f32,
    /// Ceiling on a single generation's output tokens.
    pub max_tokens: u32,
    /// Optional reproducibility seed; `None` varies each run.
    pub seed: Option<i64>,
}

impl Default for ArtifactSettings {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            max_tokens: 2048,
            seed: Some(1234),
        }
    }
}

/// The full persisted settings document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    pub appearance: AppearanceSettings,
    pub provider: ProviderSettings,
    pub artifacts: ArtifactSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            appearance: AppearanceSettings::default(),
            provider: ProviderSettings::default(),
            artifacts: ArtifactSettings::default(),
        }
    }
}

/// Optional fields to merge into [`AppSettings::appearance`].
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AppearanceSettingsPatch {
    pub theme: Option<ThemePreference>,
}

/// Optional fields to merge into [`AppSettings::artifacts`]. A `seed` of
/// `Some(None)` explicitly clears the saved seed; a missing field leaves it
/// untouched.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ArtifactSettingsPatch {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub seed: Option<Option<i64>>,
}

/// Partial settings update. Provider configuration is deliberately not part of
/// the patch: it flows through `set_provider_config` so the preset-pinning and
/// keychain behavior stays in one place.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AppSettingsPatch {
    pub appearance: Option<AppearanceSettingsPatch>,
    pub artifacts: Option<ArtifactSettingsPatch>,
}

/// Process-wide holder of [`AppSettings`] with atomic JSON persistence.
pub struct SettingsState {
    storage: RwLock<AppSettings>,
    path: PathBuf,
}

impl SettingsState {
    /// Load settings from `path`, falling back to defaults if the file does
    /// not exist yet. The parent directory is created on demand.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create settings dir: {e}"))?;
        }
        let storage = match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|e| format!("Failed to parse settings at {}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AppSettings::default(),
            Err(e) => return Err(format!("Failed to read settings at {}: {e}", path.display())),
        };
        Ok(Self {
            storage: RwLock::new(storage),
            path,
        })
    }

    /// The current settings.
    pub fn get(&self) -> AppSettings {
        self.storage.read().unwrap().clone()
    }

    /// Merge an optional patch and persist the result. The merge and write
    /// happen under the write lock so concurrent updates cannot drop each
    /// other's fields.
    pub fn update(&self, patch: &AppSettingsPatch) -> Result<AppSettings, String> {
        let mut settings = self.storage.write().unwrap();
        if let Some(appearance) = &patch.appearance {
            settings.appearance = AppearanceSettings {
                theme: appearance.theme.unwrap_or(settings.appearance.theme),
            };
        }
        if let Some(artifacts) = &patch.artifacts {
            settings.artifacts = ArtifactSettings {
                temperature: artifacts.temperature.unwrap_or(settings.artifacts.temperature),
                max_tokens: artifacts.max_tokens.unwrap_or(settings.artifacts.max_tokens),
                seed: artifacts.seed.unwrap_or(settings.artifacts.seed),
            };
        }
        self.persist_locked(&settings)?;
        Ok(settings.clone())
    }

    /// Update a single preset's config and set it as active. The read-modify-write
    /// happens under the write lock so concurrent callers cannot drop each
    /// other's changes.
    pub fn update_provider_preset(
        &self,
        preset: &str,
        config: PerPresetConfig,
    ) -> Result<AppSettings, String> {
        let mut settings = self.storage.write().unwrap();
        settings.provider.active = preset.to_string();
        settings.provider.presets.insert(preset.to_string(), config);
        self.persist_locked(&settings)?;
        Ok(settings.clone())
    }

    /// Atomically replace the settings file (temp write + rename).
    fn persist_locked(&self, settings: &AppSettings) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(settings)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("Failed to write settings: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("Failed to replace settings: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("worxheet-settings-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn test_load_returns_defaults_when_file_missing() {
        let state = SettingsState::load(temp_path()).unwrap();
        let settings = state.get();
        assert_eq!(settings.appearance.theme, ThemePreference::System);
        assert_eq!(settings.artifacts.temperature, 0.7);
        assert_eq!(settings.artifacts.max_tokens, 2048);
        assert_eq!(settings.artifacts.seed, Some(1234));
        assert_eq!(settings.provider.active, "openai");
        assert!(settings.provider.active_config().disable_thinking);
    }

    #[test]
    fn test_update_merges_and_persists() {
        let path = temp_path();
        let state = SettingsState::load(path.clone()).unwrap();
        state
            .update(&AppSettingsPatch {
                appearance: Some(AppearanceSettingsPatch {
                    theme: Some(ThemePreference::Dark),
                }),
                artifacts: Some(ArtifactSettingsPatch {
                    temperature: Some(0.5),
                    max_tokens: Some(1024),
                    seed: None,
                }),
            })
            .unwrap();

        let reloaded = SettingsState::load(path).unwrap().get();
        assert_eq!(reloaded.appearance.theme, ThemePreference::Dark);
        assert_eq!(reloaded.artifacts.temperature, 0.5);
        assert_eq!(reloaded.artifacts.max_tokens, 1024);
        assert_eq!(reloaded.artifacts.seed, Some(1234));
        assert_eq!(reloaded.provider.active, "openai");
    }

    #[test]
    fn test_seed_can_be_explicitly_cleared() {
        let path = temp_path();
        let state = SettingsState::load(path.clone()).unwrap();
        state
            .update(&AppSettingsPatch {
                appearance: None,
                artifacts: Some(ArtifactSettingsPatch {
                    temperature: None,
                    max_tokens: None,
                    seed: Some(None),
                }),
            })
            .unwrap();
        assert_eq!(SettingsState::load(path).unwrap().get().artifacts.seed, None);
    }

    #[test]
    fn test_update_provider_preset_persists_and_round_trips() {
        let path = temp_path();
        let state = SettingsState::load(path.clone()).unwrap();
        let config = PerPresetConfig {
            base_url: "https://ollama.com/v1".to_string(),
            model: "llama3.1".to_string(),
            concurrency: 4,
            disable_thinking: true,
        };
        state.update_provider_preset("ollama", config.clone()).unwrap();

        let reloaded = SettingsState::load(path).unwrap().get();
        assert_eq!(reloaded.provider.active, "ollama");
        let ollama = reloaded.provider.presets.get("ollama").unwrap();
        assert_eq!(ollama.base_url, config.base_url);
        assert_eq!(ollama.model, config.model);
        assert_eq!(ollama.concurrency, 4);
        // openai defaults are still there
        assert!(reloaded.provider.presets.contains_key("openai"));
    }

    #[test]
    fn test_corrupt_file_fails_loudly() {
        let path = temp_path();
        std::fs::write(&path, b"{not json").unwrap();
        let result = SettingsState::load(path);
        assert!(result.is_err());
    }
}