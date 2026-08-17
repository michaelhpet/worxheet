use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::data::LlamaTokenData;
use llama_cpp_2::token::data_array::LlamaTokenDataArray;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::json_schema_to_grammar;
use serde::Deserialize;
use tokenizers::Tokenizer;

pub const EMBED_MODEL_REPO: &str = "ggml-org/bge-small-en-v1.5-Q8_0-GGUF";
pub const EMBED_MODEL_FILE: &str = "bge-small-en-v1.5-q8_0.gguf";

pub const GENERATION_MODEL_REPO: &str = "HuggingFaceTB/SmolLM2-360M-Instruct-GGUF";
pub const GENERATION_MODEL_FILE: &str = "smollm2-360m-instruct-q8_0.gguf";

pub const TOKENIZER_REPO: &str = "HuggingFaceTB/SmolLM2-360M-Instruct";
pub const TOKENIZER_FILE: &str = "tokenizer.json";

const N_CTX: u32 = 8192;
const MAX_PROMPT_TOKENS: i32 = 6144;

static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();

/// The process-wide llama.cpp backend.
///
/// `llama_backend_init` may only be called once per process, and `LlamaBackend`
/// tears the backend down on drop. To respect both constraints we initialize it
/// exactly once and keep it alive for the entire process, handing out `&'static`
/// references from which models and contexts are created.
pub fn backend() -> Result<&'static LlamaBackend, String> {
    match BACKEND.get_or_init(|| LlamaBackend::init().map_err(|e| e.to_string())) {
        Ok(backend) => Ok(backend),
        Err(error) => Err(error.clone()),
    }
}

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

/// Wraps the `bge-small-en-v1.5` GGUF model and produces L2-normalized text
/// embeddings via llama.cpp.
pub struct Embedder {
    model: LlamaModel,
}

impl Embedder {
    pub fn load(model_path: &Path) -> Result<Self, String> {
        let backend = backend()?;
        let model = LlamaModel::load_from_file(backend, model_path, &LlamaModelParams::default())
            .map_err(|e| format!("Failed to load embedding model: {e}"))?;
        Ok(Self { model })
    }

    /// Embedding vector width of the loaded model.
    #[allow(dead_code)] // used by tests and useful for consumers
    pub fn dimension(&self) -> usize {
        self.model.n_embd() as usize
    }

    /// Embed a batch of texts. Each text is embedded independently and the
    /// resulting vectors are L2-normalized (unit norm), suitable for cosine
    /// similarity.
    ///
    /// Texts are embedded one per decode call: llama.cpp's scheduler ubatch only
    /// supports a single sequence per batch, so multi-sequence batches are not
    /// used here (mirroring llama.cpp's own `embedding` example).
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let n_ctx_train = self.model.n_ctx_train();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);
        let ctx_params = LlamaContextParams::default()
            .with_embeddings(true)
            .with_n_ctx(NonZeroU32::new(n_ctx_train))
            .with_n_threads_batch(threads);
        let mut ctx = self
            .model
            .new_context(backend()?, ctx_params)
            .map_err(|e| format!("Failed to create embedding context: {e}"))?;

        let n_ctx = ctx.n_ctx() as usize;
        let mut output = Vec::with_capacity(texts.len());

        for text in texts {
            let mut tokens = self
                .model
                .str_to_token(text, AddBos::Always)
                .map_err(|e| format!("Failed to tokenize text: {e}"))?;
            // Chunks are sized up to 512 tokens, but re-tokenization and the
            // BOS token can push them past the model's context window. Truncate
            // from the end instead of failing so every chunk is embeddable.
            if tokens.len() > n_ctx {
                tokens.truncate(n_ctx);
            }

            let mut batch = LlamaBatch::new(n_ctx, 1);
            batch
                .add_sequence(&tokens, 0, false)
                .map_err(|e| format!("Failed to add sequence to batch: {e}"))?;

            ctx.clear_kv_cache();
            ctx.decode(&mut batch)
                .map_err(|e| format!("Embedding decode failed: {e}"))?;

            let embedding = ctx
                .embeddings_seq_ith(0)
                .map_err(|e| format!("Failed to read embedding: {e}"))?;
            output.push(normalize(embedding));
        }

        Ok(output)
    }
}

fn normalize(input: &[f32]) -> Vec<f32> {
    let magnitude = input
        .iter()
        .fold(0.0f32, |acc, &value| acc + value * value)
        .sqrt();
    if magnitude == 0.0 {
        return vec![0.0; input.len()];
    }
    input.iter().map(|&value| value / magnitude).collect()
}

/// Wraps the `SmolLM2-360M-Instruct` GGUF model for artifact generation.
pub struct Generator {
    model: LlamaModel,
}

#[derive(Clone, Deserialize)]
pub struct GenerationParams {
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: i32,
    pub seed: u32,
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 1024,
            seed: 1234,
        }
    }
}

impl Generator {
    pub fn load(model_path: &Path) -> Result<Self, String> {
        let backend = backend()?;
        let model = LlamaModel::load_from_file(backend, model_path, &LlamaModelParams::default())
            .map_err(|e| format!("Failed to load generation model: {e}"))?;
        Ok(Self { model })
    }

    /// Apply the model's built-in chat template to a system + user message pair,
    /// producing a prompt ready for generation.
    pub fn apply_chat_template(&self, system: &str, user: &str) -> Result<String, String> {
        let template = self
            .model
            .chat_template(None)
            .map_err(|e| format!("Model has no chat template: {e}"))?;

        let messages = vec![
            LlamaChatMessage::new("system".to_string(), system.to_string())
                .map_err(|e| format!("Invalid system message: {e}"))?,
            LlamaChatMessage::new("user".to_string(), user.to_string())
                .map_err(|e| format!("Invalid user message: {e}"))?,
        ];

        self.model
            .apply_chat_template(&template, &messages, true)
            .map_err(|e| format!("Failed to apply chat template: {e}"))
    }

    /// Run a generation with an optional JSON-schema grammar constraining the
    /// output to valid JSON matching the schema.
    pub fn generate(
        &self,
        prompt: &str,
        schema_json: Option<&str>,
        params: &GenerationParams,
    ) -> Result<String, String> {
        let n_ctx = NonZeroU32::new(N_CTX).expect("non-zero context size");
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(n_ctx))
            .with_n_batch(N_CTX);
        let mut ctx = self
            .model
            .new_context(backend()?, ctx_params)
            .map_err(|e| format!("Failed to create generation context: {e}"))?;

        let tokens = self
            .model
            .str_to_token(prompt, AddBos::Always)
            .map_err(|e| format!("Failed to tokenize prompt: {e}"))?;
        if tokens.len() > MAX_PROMPT_TOKENS as usize {
            return Err(format!(
                "Prompt too long ({} > {} tokens)",
                tokens.len(),
                MAX_PROMPT_TOKENS
            ));
        }

        // Cap generation so the total sequence never overflows the context
        // window, leaving one slot for the token currently being decoded.
        let prompt_tokens = tokens.len() as i32;
        let budget = params.max_tokens.min(N_CTX as i32 - prompt_tokens - 1);
        if budget <= 0 {
            return Err(format!(
                "Prompt fills the {N_CTX}-token context window ({} prompt tokens leave no room for output)",
                prompt_tokens
            ));
        }

        let mut batch = LlamaBatch::new(tokens.len().max(1), 1);
        let last_index = (tokens.len() - 1) as i32;
        for (i, token) in (0..).zip(&tokens) {
            batch
                .add(*token, i, &[0], i == last_index)
                .map_err(|e| format!("Failed to add prompt token: {e}"))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| format!("Failed to decode prompt: {e}"))?;

        let mut samplers: Vec<LlamaSampler> = Vec::new();
        if let Some(schema) = schema_json {
            let grammar = json_schema_to_grammar(schema)
                .map_err(|e| format!("Failed to compile JSON schema into grammar: {e}"))?;
            samplers.push(
                LlamaSampler::grammar(&self.model, &grammar, "root")
                    .map_err(|e| format!("Failed to initialize grammar sampler: {e}"))?,
            );
        }
        if params.temperature > 0.0 {
            samplers.push(LlamaSampler::temp(params.temperature));
        }
        samplers.push(LlamaSampler::top_p(params.top_p, 1));
        samplers.push(LlamaSampler::dist(params.seed));
        let mut sampler = LlamaSampler::chain_simple(samplers);

        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut output = String::new();
        let mut n_cur = batch.n_tokens();
        let mut generated = 0i32;

        while generated < budget {
            let idx = batch.n_tokens() - 1;
            let logits = ctx.get_logits_ith(idx);
            let mut data_array = LlamaTokenDataArray::from_iter(
                logits
                    .iter()
                    .enumerate()
                    .map(|(i, &l)| LlamaTokenData::new(LlamaToken(i as i32), l, 0.0)),
                false,
            );
            data_array.apply_sampler(&sampler);
            let Some(token) = data_array.selected_token() else {
                break;
            };
            if self.model.is_eog_token(token) {
                break;
            }
            sampler.accept(token);

            if let Ok(piece) = self.model.token_to_piece(token, &mut decoder, true, None) {
                output.push_str(&piece);
            }

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| format!("Failed to add generated token: {e}"))?;
            n_cur += 1;
            generated += 1;
            ctx.decode(&mut batch)
                .map_err(|e| format!("Failed to decode generated token: {e}"))?;
        }

        Ok(output.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn models_dir() -> PathBuf {
        std::env::var("WORXHEET_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/models")))
    }

    // --- Embedder tests ---

    #[test]
    fn test_embed_dimension() {
        let path = models_dir().join("bge-small-en-v1.5-q8_0.gguf");
        if !path.exists() {
            eprintln!("skipping: embedding model not found at {}", path.display());
            return;
        }
        let embedder = Embedder::load(&path).expect("should load model");
        assert_eq!(embedder.dimension(), 384);
    }

    #[test]
    fn test_embed_similarity() {
        let path = models_dir().join("bge-small-en-v1.5-q8_0.gguf");
        if !path.exists() {
            eprintln!("skipping: embedding model not found at {}", path.display());
            return;
        }
        let embedder = Embedder::load(&path).expect("should load model");

        let vectors = embedder
            .embed(&[
                "Photosynthesis converts sunlight into chemical energy.",
                "The mitochondria is the powerhouse of the cell.",
                "Photosynthesis occurs in the chloroplast of plant cells.",
            ])
            .expect("should embed");

        assert_eq!(vectors.len(), 3);
        for vector in &vectors {
            assert_eq!(vector.len(), 384);
            let norm: f32 = vector.iter().map(|v| v * v).sum();
            assert!((norm - 1.0).abs() < 1e-3, "vectors should be normalized");
        }

        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let photo_photo = dot(&vectors[0], &vectors[2]);
        let photo_mito = dot(&vectors[0], &vectors[1]);
        assert!(
            photo_photo > photo_mito,
            "photosynthesis chunks should be closer to each other than to the mitochondria chunk"
        );
    }

    #[test]
    fn test_embed_empty() {
        let path = models_dir().join("bge-small-en-v1.5-q8_0.gguf");
        if !path.exists() {
            eprintln!("skipping: embedding model not found at {}", path.display());
            return;
        }
        let embedder = Embedder::load(&path).expect("should load model");
        let result = embedder.embed(&[]).expect("empty batch should succeed");
        assert!(result.is_empty());
    }

    #[test]
    fn test_embed_long_text_truncates() {
        let path = models_dir().join("bge-small-en-v1.5-q8_0.gguf");
        if !path.exists() {
            eprintln!("skipping: embedding model not found at {}", path.display());
            return;
        }
        let embedder = Embedder::load(&path).expect("should load model");

        // ~700 tokens of plain text, well past the 512-token window.
        let long_text = "the mitochondrion is the powerhouse of the cell ".repeat(700);
        let vectors = embedder
            .embed(&[&long_text])
            .expect("long text should embed (truncated) instead of erroring");

        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].len(), 384);
        let norm: f32 = vectors[0].iter().map(|v| v * v).sum();
        assert!((norm - 1.0).abs() < 1e-3, "vectors should be normalized");
    }

    // --- Generator tests ---

    #[test]
    fn test_chat_template() {
        let path = models_dir().join("smollm2-360m-instruct-q8_0.gguf");
        if !path.exists() {
            eprintln!("skipping: generation model not found at {}", path.display());
            return;
        }
        let generator = Generator::load(&path).expect("should load model");
        let prompt = generator
            .apply_chat_template(
                "You are a helpful assistant.",
                "What is 2 + 2? Answer with a single number.",
            )
            .expect("should apply chat template");
        assert!(prompt.contains("<|im_start|>") || prompt.contains("<|user|>"));
    }

    #[test]
    fn test_generate_grammar_json() {
        let path = models_dir().join("smollm2-360m-instruct-q8_0.gguf");
        if !path.exists() {
            eprintln!("skipping: generation model not found at {}", path.display());
            return;
        }
        let generator = Generator::load(&path).expect("should load model");

        let system = "You are an educational assessment generator. Reply only with valid JSON.";
        let user =
            "Based on the passage 'The cell is the basic unit of life.', generate one quiz question.";
        let prompt = generator
            .apply_chat_template(system, user)
            .expect("should build prompt");

        let schema = r#"{
            "type": "object",
            "properties": {
                "question": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" }, "minItems": 4, "maxItems": 4 }
            },
            "required": ["question", "options"]
        }"#;

        let params = GenerationParams {
            temperature: 0.3,
            max_tokens: 256,
            ..Default::default()
        };

        let output = generator
            .generate(&prompt, Some(schema), &params)
            .expect("should generate");

        let parsed: serde_json::Value = serde_json::from_str(&output)
            .map_err(|e| format!("model did not return valid JSON: {e}; got: {output}"))
            .expect("output should parse as JSON");

        assert!(parsed.get("question").is_some());
        assert_eq!(parsed["options"].as_array().map(Vec::len), Some(4));
    }
}