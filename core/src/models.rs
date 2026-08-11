use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokenizers::Tokenizer;

use crate::embed::Embedder;
use crate::generation::Generator;

pub const EMBED_MODEL_REPO: &str = "ggml-org/bge-small-en-v1.5-Q8_0-GGUF";
pub const EMBED_MODEL_FILE: &str = "bge-small-en-v1.5-q8_0.gguf";

pub const GENERATION_MODEL_REPO: &str = "HuggingFaceTB/SmolLM2-360M-Instruct-GGUF";
pub const GENERATION_MODEL_FILE: &str = "smollm2-360m-instruct-q8_0.gguf";

pub const TOKENIZER_REPO: &str = "HuggingFaceTB/SmolLM2-360M-Instruct";
pub const TOKENIZER_FILE: &str = "tokenizer.json";

#[derive(Clone, Debug)]
pub struct ModelPaths {
    pub embed: PathBuf,
    pub generate: PathBuf,
    pub tokenizer: PathBuf,
}

/// Lazily initialized, process-wide access to the downloaded models and
/// tokenizer. Each component is downloaded (if missing) and loaded exactly
/// once, then reused for the rest of the session.
pub struct ModelPool {
    models_dir: PathBuf,
    tokenizer: Mutex<Option<Tokenizer>>,
    embedder: Mutex<Option<Arc<Embedder>>>,
    generator: Mutex<Option<Arc<Generator>>>,
}

impl ModelPool {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            tokenizer: Mutex::new(None),
            embedder: Mutex::new(None),
            generator: Mutex::new(None),
        }
    }

    /// Returns the tokenizer, downloading models and the tokenizer on first use.
    pub fn tokenizer(&self) -> Result<Tokenizer, String> {
        if let Some(tokenizer) = self.tokenizer.lock().unwrap().as_ref() {
            return Ok(tokenizer.clone());
        }
        let paths = self.paths()?;
        let tokenizer = Tokenizer::from_file(&paths.tokenizer)
            .map_err(|e| format!("Failed to load tokenizer: {e}"))?;
        *self.tokenizer.lock().unwrap() = Some(tokenizer.clone());
        Ok(tokenizer)
    }

    /// Returns the embedding model, downloading and loading it on first use.
    pub fn embedder(&self) -> Result<Arc<Embedder>, String> {
        if let Some(embedder) = self.embedder.lock().unwrap().as_ref() {
            return Ok(embedder.clone());
        }
        let paths = self.paths()?;
        let embedder = Arc::new(Embedder::load(&paths.embed)?);
        *self.embedder.lock().unwrap() = Some(embedder.clone());
        Ok(embedder)
    }

    /// Returns the generation model, downloading and loading it on first use.
    pub fn generator(&self) -> Result<Arc<Generator>, String> {
        if let Some(generator) = self.generator.lock().unwrap().as_ref() {
            return Ok(generator.clone());
        }
        let paths = self.paths()?;
        let generator = Arc::new(Generator::load(&paths.generate)?);
        *self.generator.lock().unwrap() = Some(generator.clone());
        Ok(generator)
    }

    fn paths(&self) -> Result<ModelPaths, String> {
        ensure_downloaded(&self.models_dir)
    }
}

pub fn models_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models")
}

/// Ensure all required artifacts are present under `models_dir`, downloading
/// them on first launch, and return their local paths.
pub fn ensure_downloaded(models_dir: &Path) -> Result<ModelPaths, String> {
    std::fs::create_dir_all(models_dir).map_err(|e| format!("Failed to create models dir: {e}"))?;

    let embed = download_if_missing(models_dir, EMBED_MODEL_REPO, EMBED_MODEL_FILE)?;
    let generate = download_if_missing(models_dir, GENERATION_MODEL_REPO, GENERATION_MODEL_FILE)?;
    let tokenizer = download_if_missing(models_dir, TOKENIZER_REPO, TOKENIZER_FILE)?;

    Ok(ModelPaths {
        embed,
        generate,
        tokenizer,
    })
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
