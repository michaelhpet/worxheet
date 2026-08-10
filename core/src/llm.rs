use std::sync::OnceLock;

use llama_cpp_2::llama_backend::LlamaBackend;

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
