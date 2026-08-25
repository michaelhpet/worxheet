//! Local embedding model: `nomic-embed-text-v1.5` (int8-quantized ONNX).
//!
//! The ONNX artifact (~137MB) is fetched once from a pinned Hugging Face
//! mirror into the app data directory; the tokenizer is vendored into the
//! binary so ingest works fully offline after that one-time download.
//! Inference runs on CPU through `ort` with the nomic recipe: document task
//! prefix, mean pooling over the attention mask, L2 normalization.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

/// Pinned int8 ONNX export of nomic-embed-text-v1.5 (~137MB).
const MODEL_URL: &str =
    "https://huggingface.co/MidnightPhreaker/nomic-embed-text-v1.5-onnx/resolve/main/onnx/model_int8.onnx";
pub const MODEL_FILE_NAME: &str = "nomic-embed-text-v1.5-int8.onnx";

/// Nomic v1.5 expects a task prefix describing how texts will be consumed.
const DOCUMENT_PREFIX: &str = "search_document: ";

/// Sub-token budget fed to the encoder per text. Drift windows are short;
/// capping here keeps batches fast regardless of input length.
const MAX_INPUT_TOKENS: usize = 256;

/// Texts per ORT run.
const BATCH_SIZE: usize = 16;

const EMBED_THREADS: usize = 4;

fn progress_interval() -> std::time::Duration {
    std::time::Duration::from_millis(100)
}

/// Byte-range progress while the embedding model downloads.
pub type DownloadProgress = Arc<dyn Fn(u64, u64) + Send + Sync>;

pub fn models_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models")
}

async fn ensure_model_file(
    dir: &Path,
    on_progress: Option<DownloadProgress>,
) -> Result<PathBuf, String> {
    let destination = dir.join(MODEL_FILE_NAME);
    if destination.is_file() {
        return Ok(destination);
    }

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("Failed to create models dir: {e}"))?;

    let response = reqwest::get(MODEL_URL)
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|e| format!("Failed to download embedding model: {e}"))?;

    let total = response.content_length().unwrap_or(0);
    if let Some(on_progress) = &on_progress {
        on_progress(0, total);
    }

    use tokio::io::AsyncWriteExt;

    let partial = destination.with_extension("part");
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|e| format!("Failed to create {}: {e}", partial.display()))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Failed to write download buffer: {e}"))?;
        downloaded += chunk.len() as u64;
        if let Some(on_progress) = &on_progress {
            if last_emit.elapsed() >= progress_interval() {
                last_emit = std::time::Instant::now();
                on_progress(downloaded, total);
            }
        }
    }
    file.flush()
        .await
        .map_err(|e| format!("Failed to flush download buffer: {e}"))?;
    drop(file);

    if let Some(on_progress) = &on_progress {
        on_progress(downloaded, total);
    }

    tokio::fs::rename(&partial, &destination)
        .await
        .map_err(|e| format!("Failed to finalize embedding model: {e}"))?;

    Ok(destination)
}

fn load_tokenizer() -> Result<Tokenizer, String> {
    const TOKENIZER_JSON: &str = include_str!("../assets/tokenizer.json");
    Tokenizer::from_bytes(TOKENIZER_JSON)
        .map_err(|e| format!("Failed to parse bundled tokenizer: {e}"))
}

/// Parse the vendored tokenizer (also used by tests that need real
/// token-count semantics without downloading the model).
pub fn bundled_tokenizer() -> Result<Tokenizer, String> {
    load_tokenizer()
}

/// Lazy process-wide access to the loaded embedder. The first successful load
/// is reused for the rest of the session; failures are retried on demand.
pub struct EmbedderPool {
    inner: Mutex<Option<Arc<Embedder>>>,
    on_download: Option<DownloadProgress>,
}

impl EmbedderPool {
    /// `on_download` receives byte-range progress during the one-time model
    /// fetch.
    pub fn new(on_download: Option<DownloadProgress>) -> Self {
        Self {
            inner: Mutex::new(None),
            on_download,
        }
    }

    /// Returns the shared embedder, downloading and loading it on first use.
    pub async fn get(&self, models_dir: &Path) -> Result<Arc<Embedder>, String> {
        if let Some(embedder) = self.inner.lock().unwrap().as_ref() {
            return Ok(embedder.clone());
        }
        let embedder = Arc::new(Embedder::load(
            &ensure_model_file(models_dir, self.on_download.clone()).await?,
        )?);
        Ok(self.inner.lock().unwrap().get_or_insert(embedder).clone())
    }
}

/// Runs `nomic-embed-text-v1.5` through ONNX Runtime.
pub struct Embedder {
    tokenizer: Tokenizer,
    session: Mutex<Session>,
}

impl Embedder {
    pub fn load(model_path: &Path) -> Result<Self, String> {
        let mut tokenizer = load_tokenizer()?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_INPUT_TOKENS,
                ..tokenizers::TruncationParams::default()
            }))
            .map_err(|e| format!("Failed to configure tokenizer truncation: {e}"))?;

        let session = Session::builder()
            .map_err(|e| format!("Failed to create ORT session builder: {e}"))?
            .with_intra_threads(EMBED_THREADS)
            .map_err(|e| format!("Failed to set ORT threads: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| {
                format!(
                    "Failed to load embedding model {}: {e}",
                    model_path.display()
                )
            })?;

        Ok(Self {
            tokenizer,
            session: Mutex::new(session),
        })
    }

    /// Embed texts with the document prefix applied; outputs are
    /// L2-normalized vectors in input order.
    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let encodings: Vec<Vec<u32>> = texts
            .iter()
            .map(|text| {
                self.tokenizer
                    .encode(format!("{DOCUMENT_PREFIX}{text}"), true)
                    .map(|encoding| encoding.get_ids().to_vec())
                    .map_err(|e| format!("Tokenization failed: {e}"))
            })
            .collect::<Result<_, _>>()?;

        let mut output = Vec::with_capacity(texts.len());
        for batch in encodings.chunks(BATCH_SIZE) {
            output.extend(self.embed_batch(batch)?);
        }
        Ok(output)
    }

    fn embed_batch(&self, batch: &[Vec<u32>]) -> Result<Vec<Vec<f32>>, String> {
        let count = batch.len();
        let max_len = batch.iter().map(Vec::len).max().unwrap_or(0);
        let shape = vec![count as i64, max_len as i64];

        let mut input_ids = vec![0i64; count * max_len];
        let mut attention_mask = vec![0i64; count * max_len];
        for (row, tokens) in batch.iter().enumerate() {
            for (col, token) in tokens.iter().enumerate() {
                input_ids[row * max_len + col] = *token as i64;
                attention_mask[row * max_len + col] = 1;
            }
        }

        let ids_tensor = Tensor::from_array((shape.clone(), input_ids))
            .map_err(|e| format!("input_ids tensor: {e}"))?;
        let mask_tensor = Tensor::from_array((shape.clone(), attention_mask))
            .map_err(|e| format!("attention_mask tensor: {e}"))?;

        let needs_token_type = self
            .session
            .lock()
            .unwrap()
            .inputs
            .iter()
            .any(|input| input.name == "token_type_ids");

        let mut session = self.session.lock().unwrap();
        let outputs = if needs_token_type {
            let types_tensor = Tensor::from_array((shape, vec![0i64; count * max_len]))
                .map_err(|e| format!("token_type_ids tensor: {e}"))?;
            session
                .run(ort::inputs![
                    "input_ids" => ids_tensor,
                    "attention_mask" => mask_tensor,
                    "token_type_ids" => types_tensor,
                ])
                .map_err(|e| format!("Embedding inference failed: {e}"))?
        } else {
            session
                .run(ort::inputs![
                    "input_ids" => ids_tensor,
                    "attention_mask" => mask_tensor,
                ])
                .map_err(|e| format!("Embedding inference failed: {e}"))?
        };

        // Mean-pool last hidden states over unmasked positions.
        let (_, value) = outputs
            .iter()
            .next()
            .ok_or_else(|| String::from("Embedding model returned no outputs"))?;
        let (tensor_shape, hidden) = value
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to read embedding tensor: {e}"))?;
        if tensor_shape.len() != 3 {
            return Err(format!(
                "Unexpected embedding tensor rank {} (expected 3)",
                tensor_shape.len()
            ));
        }
        let dim = tensor_shape[2] as usize;

        let mut pooled = Vec::with_capacity(count);
        for (row, tokens) in batch.iter().enumerate() {
            let mut sum = vec![0f32; dim];
            let used = tokens.len() as f32;
            for col in 0..tokens.len() {
                let offset = (row * max_len + col) * dim;
                for (acc, value) in sum.iter_mut().zip(&hidden[offset..offset + dim]) {
                    *acc += value;
                }
            }
            if used > 0.0 {
                for value in &mut sum {
                    *value /= used;
                }
            }
            normalize_in_place(&mut sum);
            pooled.push(sum);
        }

        Ok(pooled)
    }
}

fn normalize_in_place(vector: &mut [f32]) {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in vector.iter_mut() {
            *value /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_zero_vector_is_safe() {
        let mut vector = vec![0.0f32; 4];
        normalize_in_place(&mut vector);
        assert!(vector.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn test_bundled_tokenizer_loads_and_truncates() {
        let mut tokenizer = load_tokenizer().expect("bundled tokenizer should parse");
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: 32,
                ..tokenizers::TruncationParams::default()
            }))
            .unwrap();
        let text = "word ".repeat(500);
        let encoding = tokenizer.encode(text, true).unwrap();
        assert_eq!(encoding.get_ids().len(), 32);
    }

    #[tokio::test]
    async fn test_embedder_pool_loads_and_embeds() {
        let dir = std::env::temp_dir().join("worxheet-embed-test-models");
        let pool = EmbedderPool::new(None);
        let Ok(embedder) = pool.get(&dir).await else {
            eprintln!("skipping: embedding model unavailable");
            return;
        };

        let vectors = embedder
            .embed(&[
                String::from("Photosynthesis converts light into chemical energy."),
                String::from("Mitochondria produce ATP through cellular respiration."),
            ])
            .expect("embedding should succeed");
        assert_eq!(vectors.len(), 2);
        for vector in &vectors {
            let norm: f32 = vector.iter().map(|v| v * v).sum();
            assert!((norm - 1.0).abs() < 1e-2, "expected unit norm, got {norm}");
        }
    }
}
