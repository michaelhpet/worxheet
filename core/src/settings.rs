//! App-wide settings persisted as JSON; writes flush atomically (temp + rename).

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::provider::config::{PerPresetConfig, ProviderSettings};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: ThemePreference,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArtifactSettings {
    pub temperature: f32,
    pub max_tokens: u32,
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppSettings {
    pub appearance: AppearanceSettings,
    pub provider: ProviderSettings,
    pub artifacts: ArtifactSettings,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AppearanceSettingsPatch {
    pub theme: Option<ThemePreference>,
}

/// `seed: Some(None)` clears the saved seed; a missing field leaves it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ArtifactSettingsPatch {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub seed: Option<Option<i64>>,
}

/// Provider configuration is deliberately excluded: it flows through
/// `set_provider_config` instead.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AppSettingsPatch {
    pub appearance: Option<AppearanceSettingsPatch>,
    pub artifacts: Option<ArtifactSettingsPatch>,
}

pub struct SettingsState {
    storage: RwLock<AppSettings>,
    path: PathBuf,
}

impl SettingsState {
    /// Falls back to defaults when the file does not exist yet.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create settings dir: {e}"))?;
        }
        let storage = match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|e| format!("Failed to parse settings at {}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AppSettings::default(),
            Err(e) => {
                return Err(format!(
                    "Failed to read settings at {}: {e}",
                    path.display()
                ))
            }
        };
        Ok(Self {
            storage: RwLock::new(storage),
            path,
        })
    }

    pub fn get(&self) -> AppSettings {
        self.storage.read().unwrap().clone()
    }

    /// Merge a patch and persist; the write lock keeps concurrent updates whole.
    pub fn update(&self, patch: &AppSettingsPatch) -> Result<AppSettings, String> {
        let mut settings = self.storage.write().unwrap();
        if let Some(appearance) = &patch.appearance {
            settings.appearance = AppearanceSettings {
                theme: appearance.theme.unwrap_or(settings.appearance.theme),
            };
        }
        if let Some(artifacts) = &patch.artifacts {
            settings.artifacts = ArtifactSettings {
                temperature: artifacts
                    .temperature
                    .unwrap_or(settings.artifacts.temperature),
                max_tokens: artifacts
                    .max_tokens
                    .unwrap_or(settings.artifacts.max_tokens),
                seed: artifacts.seed.unwrap_or(settings.artifacts.seed),
            };
        }
        self.persist_locked(&settings)?;
        Ok(settings.clone())
    }

    /// Update one preset's config and set it active.
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

    fn persist_locked(&self, settings: &AppSettings) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(settings)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("Failed to write settings: {e}"))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("Failed to replace settings: {e}"))?;
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
        assert_eq!(
            SettingsState::load(path).unwrap().get().artifacts.seed,
            None
        );
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
        state
            .update_provider_preset("ollama", config.clone())
            .unwrap();

        let reloaded = SettingsState::load(path).unwrap().get();
        assert_eq!(reloaded.provider.active, "ollama");
        let ollama = reloaded.provider.presets.get("ollama").unwrap();
        assert_eq!(ollama.base_url, config.base_url);
        assert_eq!(ollama.model, config.model);
        assert_eq!(ollama.concurrency, 4);
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
