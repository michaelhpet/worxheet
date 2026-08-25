//! Provider configuration commands: settings, keychain, validation.

use serde::Serialize;
use tauri::State;

use crate::provider::{client::OpenAiClient, ArtifactBackend};
use crate::provider::config::{self, ProviderConfig};
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
    let api_key_set = if config.requires_api_key() {
        config::load_api_key()?.is_some()
    } else {
        true
    };
    Ok(ProviderStatus { config, api_key_set })
}

/// Save provider settings. `api_key` of `Some("")` clears the stored key;
/// `None` leaves it untouched.
#[tauri::command]
pub async fn set_provider_config(
    state: State<'_, AppState>,
    preset: String,
    base_url: String,
    model: String,
    concurrency: usize,
    api_key: Option<String>,
) -> Result<ProviderStatus, String> {
    let mut config = ProviderConfig {
        preset,
        base_url,
        model,
        concurrency: concurrency.clamp(1, 32),
    };
    if let Some(base_url) = config::base_url_for_preset(&config.preset) {
        // Re-pin known presets so stale custom URLs cannot linger.
        config.base_url = base_url.to_string();
    }

    if let Some(key) = &api_key {
        config::store_api_key(key)?;
    }
    state.providers.set(config.clone());

    Ok(ProviderStatus {
        api_key_set: if config.requires_api_key() {
            config::load_api_key()?.is_some()
        } else {
            true
        },
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
    let client = OpenAiClient::new(
        &state.providers.get().base_url,
        config::load_api_key()?,
        &state.providers.get().model,
    );
    client
        .list_models()
        .await
        .map_err(|error| error.to_string())
}
