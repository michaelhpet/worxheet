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

/// Which artifact is being downloaded, used to label progress events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFileKind {
    Embed,
    Generate,
    Tokenizer,
}

impl ModelFileKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Embed => "embedding",
            Self::Generate => "generation",
            Self::Tokenizer => "tokenizer",
        }
    }
}

/// Callback invoked with `(kind, downloaded_bytes, total_bytes)` while a model
/// artifact is being downloaded.
pub type ProgressSink = dyn Fn(ModelFileKind, u64, u64) + Send + Sync;

const PROGRESS_EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Bridges `hf_hub`'s `Progress` trait to the app's progress callback. Byte
/// updates are throttled to avoid flooding the event channel on large files.
struct EmitProgress {
    kind: ModelFileKind,
    on_progress: Arc<ProgressSink>,
    done: u64,
    total: u64,
    last_emit: std::time::Instant,
}

impl EmitProgress {
    fn new(kind: ModelFileKind, on_progress: Arc<ProgressSink>) -> Self {
        Self {
            kind,
            on_progress,
            done: 0,
            total: 0,
            last_emit: std::time::Instant::now(),
        }
    }

    fn emit(&self) {
        (self.on_progress)(self.kind, self.done, self.total);
    }

    fn emit_throttled(&mut self) {
        if self.last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
            self.last_emit = std::time::Instant::now();
            self.emit();
        }
    }
}

impl hf_hub::api::Progress for EmitProgress {
    fn init(&mut self, size: usize, _filename: &str) {
        self.total = size as u64;
        self.emit();
    }

    fn update(&mut self, size: usize) {
        self.done += size as u64;
        self.emit_throttled();
    }

    fn finish(&mut self) {
        self.done = self.total;
        self.emit();
    }
}

/// Lazily initialized, process-wide access to the downloaded models and
/// tokenizer. Each component is downloaded (if missing) and loaded exactly
/// once, then reused for the rest of the session.
pub struct ModelPool {
    models_dir: PathBuf,
    on_progress: Option<Arc<ProgressSink>>,
    tokenizer: Mutex<Option<Tokenizer>>,
    embedder: Mutex<Option<Arc<Embedder>>>,
    generator: Mutex<Option<Arc<Generator>>>,
}

impl ModelPool {
    #[allow(dead_code)] // used by pipeline integration tests
    pub fn new(models_dir: PathBuf) -> Self {
        Self::with_progress(models_dir, None)
    }

    pub fn with_progress(
        models_dir: PathBuf,
        on_progress: Option<Arc<ProgressSink>>,
    ) -> Self {
        Self {
            models_dir,
            on_progress,
            tokenizer: Mutex::new(None),
            embedder: Mutex::new(None),
            generator: Mutex::new(None),
        }
    }

    /// Returns the tokenizer, downloading it on first use. Unlike the embedding
    /// and generation models, the tokenizer is only ~2MB and is all that's
    /// needed for chunking, so it is fetched independently.
    pub fn tokenizer(&self) -> Result<Tokenizer, String> {
        if let Some(tokenizer) = self.tokenizer.lock().unwrap().as_ref() {
            return Ok(tokenizer.clone());
        }
        let path = self.tokenizer_path()?;
        let tokenizer = Tokenizer::from_file(&path)
            .map_err(|e| format!("Failed to load tokenizer: {e}"))?;
        *self.tokenizer.lock().unwrap() = Some(tokenizer.clone());
        Ok(tokenizer)
    }

    /// Returns the embedding model, downloading and loading it on first use.
    pub fn embedder(&self) -> Result<Arc<Embedder>, String> {
        if let Some(embedder) = self.embedder.lock().unwrap().as_ref() {
            return Ok(embedder.clone());
        }
        let path = self.embed_path()?;
        let embedder = Arc::new(Embedder::load(&path)?);
        *self.embedder.lock().unwrap() = Some(embedder.clone());
        Ok(embedder)
    }

    /// Returns the generation model, downloading and loading it on first use.
    pub fn generator(&self) -> Result<Arc<Generator>, String> {
        if let Some(generator) = self.generator.lock().unwrap().as_ref() {
            return Ok(generator.clone());
        }
        let path = self.generate_path()?;
        let generator = Arc::new(Generator::load(&path)?);
        *self.generator.lock().unwrap() = Some(generator.clone());
        Ok(generator)
    }

    fn tokenizer_path(&self) -> Result<PathBuf, String> {
        download_if_missing(
            &self.models_dir,
            TOKENIZER_REPO,
            TOKENIZER_FILE,
            ModelFileKind::Tokenizer,
            self.on_progress.clone(),
        )
    }

    fn embed_path(&self) -> Result<PathBuf, String> {
        download_if_missing(
            &self.models_dir,
            EMBED_MODEL_REPO,
            EMBED_MODEL_FILE,
            ModelFileKind::Embed,
            self.on_progress.clone(),
        )
    }

    fn generate_path(&self) -> Result<PathBuf, String> {
        download_if_missing(
            &self.models_dir,
            GENERATION_MODEL_REPO,
            GENERATION_MODEL_FILE,
            ModelFileKind::Generate,
            self.on_progress.clone(),
        )
    }
}

pub fn models_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models")
}

/// Ensure `file` from `repo` is present under `dir`, downloading it with
/// progress reporting on first use, and return its local path.
fn download_if_missing(
    dir: &Path,
    repo: &str,
    file: &str,
    kind: ModelFileKind,
    on_progress: Option<Arc<ProgressSink>>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create models dir: {e}"))?;

    let destination = dir.join(file);
    if destination.exists() {
        return Ok(destination);
    }

    let api = hf_hub::api::sync::ApiBuilder::new()
        .build()
        .map_err(|e| format!("Failed to initialize Hugging Face client: {e}"))?;

    let progress = match on_progress {
        Some(sink) => EmitProgress::new(kind, sink),
        None => EmitProgress::new(kind, Arc::new(|_, _, _| {})),
    };

    let cached = api
        .model(repo.to_string())
        .download_with_progress(file, progress)
        .map_err(|e| format!("Failed to download {repo}/{file}: {e}"))?;

    std::fs::copy(&cached, &destination)
        .map_err(|e| format!("Failed to store model at {}: {e}", destination.display()))?;

    Ok(destination)
}
