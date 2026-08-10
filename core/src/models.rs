use std::path::{Path, PathBuf};

pub const EMBED_MODEL_REPO: &str = "ggml-org/bge-small-en-v1.5-Q8_0-GGUF";
pub const EMBED_MODEL_FILE: &str = "bge-small-en-v1.5-q8_0.gguf";

pub const GENERATION_MODEL_REPO: &str = "HuggingFaceTB/SmolLM2-360M-Instruct-GGUF";
pub const GENERATION_MODEL_FILE: &str = "smollm2-360m-instruct-q8_0.gguf";

#[derive(Clone, Debug)]
pub struct ModelPaths {
    pub embed: PathBuf,
    pub generate: PathBuf,
}

pub fn models_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models")
}

/// Ensure both GGUF models are present under `models_dir`, downloading them on
/// first launch, and return their local paths.
pub fn ensure_downloaded(models_dir: &Path) -> Result<ModelPaths, String> {
    std::fs::create_dir_all(models_dir).map_err(|e| format!("Failed to create models dir: {e}"))?;

    let embed = download_if_missing(models_dir, EMBED_MODEL_REPO, EMBED_MODEL_FILE)?;
    let generate = download_if_missing(models_dir, GENERATION_MODEL_REPO, GENERATION_MODEL_FILE)?;

    Ok(ModelPaths { embed, generate })
}

fn download_if_missing(dir: &Path, repo: &str, file: &str) -> Result<PathBuf, String> {
    let destination = dir.join(file);
    if destination.exists() {
        return Ok(destination);
    }

    let api = hf_hub::api::sync::ApiBuilder::new()
        .with_progress(true)
        .build()
        .map_err(|e| format!("Failed to initialize Hugging Face client: {e}"))?;

    let cached = api
        .model(repo.to_string())
        .get(file)
        .map_err(|e| format!("Failed to download {repo}/{file}: {e}"))?;

    std::fs::copy(&cached, &destination)
        .map_err(|e| format!("Failed to store model at {}: {e}", destination.display()))?;

    Ok(destination)
}
