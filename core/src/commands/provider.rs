//! Provider configuration commands: settings, keychain, validation.

use serde::Serialize;
use tauri::State;

use crate::provider::config::{self, ProviderConfig};
use crate::provider::{client::OpenAiClient, ArtifactBackend};
use crate::AppState;

#[derive(Serialize)]
pub struct ProviderStatus {
    pub config: ProviderConfig,
    /// Whether an API key exists in the keychain (never returns the key).
    pub api_key_set: bool,
}

#[tauri::command]
pub async fn get_provider_status(state: State<'_, AppState>) -> Result<ProviderStatus, String> {
    let config = state.providers.get();
    let api_key_set = config::load_api_key(&config.preset)?.is_some();
    Ok(ProviderStatus {
        config,
        api_key_set,
    })
}

/// Save provider settings. `api_key` of `Some("")` clears the stored key for
/// the selected provider; `None` leaves it untouched.
#[tauri::command]
pub async fn set_provider_config(
    state: State<'_, AppState>,
    preset: String,
    base_url: String,
    model: String,
    concurrency: usize,
    api_key: Option<String>,
    disable_thinking: Option<bool>,
) -> Result<ProviderStatus, String> {
    let mut config = ProviderConfig {
        preset,
        base_url,
        model,
        concurrency: concurrency.clamp(1, 32),
        disable_thinking: disable_thinking.unwrap_or(true),
    };
    if let Some(base_url) = config::base_url_for_preset(&config.preset) {
        // Re-pin known presets so stale custom URLs cannot linger.
        config.base_url = base_url.to_string();
    }

    // Persist the config first so a keychain failure below can never leave the
    // active base URL/preset stale.
    state.providers.set(config.clone());

    if let Some(key) = &api_key {
        config::store_api_key(&config.preset, key)
            .map_err(|error| format!("Failed to store API key: {error}"))?;
    }

    Ok(ProviderStatus {
        api_key_set: config::load_api_key(&config.preset)?.is_some(),
        config,
    })
}

#[derive(Serialize)]
pub struct ValidationResult {
    pub ok: bool,
    pub error: Option<String>,
    pub models: Vec<String>,
}

/// Verify the current configuration end-to-end by listing models.
#[tauri::command]
pub async fn validate_provider(state: State<'_, AppState>) -> Result<ValidationResult, String> {
    let (backend, _) = match crate::pipeline::resolve_backend(&state.providers) {
        Ok(pair) => pair,
        Err(message) => {
            return Ok(ValidationResult {
                ok: false,
                error: Some(message),
                models: Vec::new(),
            })
        }
    };

    match backend.list_models().await {
        Ok(models) => Ok(ValidationResult {
            ok: true,
            error: None,
            models,
        }),
        Err(error) => Ok(ValidationResult {
            ok: false,
            error: Some(error.to_string()),
            models: Vec::new(),
        }),
    }
}

#[tauri::command]
pub async fn list_provider_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let config = state.providers.get();
    // Key presence is the only signal: absent key → no Authorization header.
    let api_key = config::load_api_key(&config.preset)?.filter(|key| !key.is_empty());
    let client = OpenAiClient::new(&config.base_url, api_key, &config.model);
    client
        .list_models()
        .await
        .map_err(|error| error.to_string())
}
