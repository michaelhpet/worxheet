use std::path::Path;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};

use crate::llm::backend;

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

        let threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);
        let ctx_params = LlamaContextParams::default()
            .with_embeddings(true)
            .with_n_threads_batch(threads);
        let mut ctx = self
            .model
            .new_context(backend()?, ctx_params)
            .map_err(|e| format!("Failed to create embedding context: {e}"))?;

        let n_ctx = ctx.n_ctx() as usize;
        let mut output = Vec::with_capacity(texts.len());

        for text in texts {
            let tokens = self
                .model
                .str_to_token(text, AddBos::Always)
                .map_err(|e| format!("Failed to tokenize text: {e}"))?;
            if tokens.len() > n_ctx {
                return Err(format!(
                    "Text exceeds embedding context window ({} > {} tokens)",
                    tokens.len(),
                    n_ctx
                ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn models_dir() -> PathBuf {
        std::env::var("WORXHEET_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/models"))
            })
    }

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
}
