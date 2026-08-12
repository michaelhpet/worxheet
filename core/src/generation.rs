use std::num::NonZeroU32;
use std::path::Path;

use serde::Deserialize;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::data::LlamaTokenData;
use llama_cpp_2::token::data_array::LlamaTokenDataArray;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::json_schema_to_grammar;

use crate::llm::backend;
use crate::schema::ArtifactType;

const N_CTX: u32 = 8192;
const MAX_PROMPT_TOKENS: i32 = 6144;

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
            let idx = (batch.n_tokens() - 1) as i32;
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

/// The JSON array field holding per-cluster items for a question-style artifact
/// type. Whole-worksheet types (Summary, MindMap) return `None`.
pub fn items_field_for(artifact_type: &ArtifactType) -> Option<&'static str> {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz | ArtifactType::EssayQuiz => Some("questions"),
        ArtifactType::CompletionQuiz => Some("items"),
        ArtifactType::Summary | ArtifactType::MindMap => None,
    }
}

/// JSON schema constraining the generated artifact for a given type.
///
/// Question-style types wrap their items in an array so a single cluster can
/// yield one or more questions; every item is persisted as its own artifact.
/// Summary and MindMap use a single object per cluster, which the pipeline
/// concatenates into one worksheet-wide artifact.
pub fn schema_for(artifact_type: &ArtifactType) -> &'static str {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz => r#"{
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "options": { "type": "array", "items": { "type": "string" }, "minItems": 4, "maxItems": 4 },
                            "answer": { "type": "integer" },
                            "explanation": { "type": "string" }
                        },
                        "required": ["question", "options", "answer", "explanation"]
                    }
                }
            },
            "required": ["questions"]
        }"#,
        ArtifactType::EssayQuiz => r#"{
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "instructions": { "type": "string" },
                            "model_answer": { "type": "string" }
                        },
                        "required": ["question", "instructions", "model_answer"]
                    }
                }
            },
            "required": ["questions"]
        }"#,
        ArtifactType::CompletionQuiz => r#"{
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "sentence": { "type": "string" },
                            "answer": { "type": "string" },
                            "hint": { "type": "string" }
                        },
                        "required": ["sentence", "answer", "hint"]
                    }
                }
            },
            "required": ["items"]
        }"#,
        ArtifactType::Summary => r#"{
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "summary": { "type": "string" },
                "key_points": { "type": "array", "items": { "type": "string" }, "minItems": 3, "maxItems": 6 }
            },
            "required": ["title", "summary", "key_points"]
        }"#,
        ArtifactType::MindMap => r#"{
            "type": "object",
            "properties": {
                "topic": { "type": "string" },
                "branches": {
                    "type": "array",
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": { "type": "string" },
                            "children": {
                                "type": "array",
                                "maxItems": 12,
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["label", "children"]
                    }
                }
            },
            "required": ["topic", "branches"]
        }"#,
    }
}

/// System prompt used when building the chat template for a generation call.
pub fn system_prompt_for(_artifact_type: &ArtifactType) -> &'static str {
    "You are an educational assessment generator. Base every answer strictly \
     on the provided source passages. Reply only with valid JSON matching the schema."
}

/// User-facing instructions for the requested artifact type. `context` holds the
/// retrieved source chunks.
pub fn user_message_for(artifact_type: &ArtifactType, context: &str) -> String {
    let task = match artifact_type {
        ArtifactType::MultipleChoiceQuiz => {
            "Generate between one and eight multiple-choice questions testing \
             higher-order thinking (analysis, application, or evaluation), drawn \
             from the passages. Every question must have exactly 4 plausible \
             options and the index of the correct answer."
        }
        ArtifactType::EssayQuiz => {
            "Generate between one and eight essay questions requiring students to \
             explain, compare, or evaluate concepts from the passages, each with \
             clear instructions and a model answer."
        }
        ArtifactType::CompletionQuiz => {
            "Generate between one and eight fill-in-the-blank sentences drawn from \
             the passages, each with the expected answer and a hint."
        }
        ArtifactType::Summary => {
            "Write a focused summary section for this slice of the source, \
             capturing its main ideas and key points."
        }
        ArtifactType::MindMap => {
            "Extract the topic covered by this slice of the source and its major \
             branches, each branch with a short list of child concepts."
        }
    };
    format!("{task}\n\nPassages:\n{context}")
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

    #[test]
    fn test_all_schemas_compile_to_grammar() {
        for artifact_type in [
            ArtifactType::MultipleChoiceQuiz,
            ArtifactType::EssayQuiz,
            ArtifactType::CompletionQuiz,
            ArtifactType::Summary,
            ArtifactType::MindMap,
        ] {
            let schema = schema_for(&artifact_type);
            let grammar = json_schema_to_grammar(schema).unwrap_or_else(|error| {
                panic!("{artifact_type:?} schema should compile into grammar: {error}")
            });
            assert!(!grammar.is_empty());
        }
    }

    #[test]
    fn test_items_field_for() {
        assert_eq!(
            items_field_for(&ArtifactType::MultipleChoiceQuiz),
            Some("questions")
        );
        assert_eq!(items_field_for(&ArtifactType::EssayQuiz), Some("questions"));
        assert_eq!(items_field_for(&ArtifactType::CompletionQuiz), Some("items"));
        assert_eq!(items_field_for(&ArtifactType::Summary), None);
        assert_eq!(items_field_for(&ArtifactType::MindMap), None);
    }
}
