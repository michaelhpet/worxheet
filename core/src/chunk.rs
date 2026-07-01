use tokenizers::Tokenizer;

pub fn chunk_text(
    text: &str,
    chunk_size: usize,
    overlap: usize,
    tokenizer: &Tokenizer,
) -> Result<Vec<String>, String> {
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let encoding = tokenizer
        .encode(text, true)
        .map_err(|e| format!("Tokenization failed: {}", e))?;
    let token_ids = encoding.get_ids();
    let token_count = token_ids.len();

    if token_count <= chunk_size {
        return Ok(vec![text.to_string()]);
    }

    let clamped_overlap = overlap.min(chunk_size / 2);
    let stride = chunk_size - clamped_overlap;

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < token_count {
        let end = std::cmp::min(start + chunk_size, token_count);
        let chunk_token_ids = &token_ids[start..end];
        let chunk_text = tokenizer
            .decode(chunk_token_ids, true)
            .map_err(|e| format!("Decoding failed: {}", e))?;
        chunks.push(chunk_text);

        if end == token_count {
            break;
        }

        start += stride;
    }

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::models::bpe::BPE;
    use tokenizers::Tokenizer;

    fn test_tokenizer() -> Tokenizer {
        let bpe = BPE::builder()
            .vocab_and_merges([("<unk>".to_string(), 0u32)], vec![])
            .unk_token("<unk>".to_string())
            .build()
            .expect("Failed to create test BPE model");

        Tokenizer::new(bpe)
    }

    #[test]
    fn test_empty_text() {
        let tokenizer = test_tokenizer();
        let result = chunk_text("", 512, 128, &tokenizer).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_short_text() {
        let tokenizer = test_tokenizer();
        let result = chunk_text("Hello world", 512, 128, &tokenizer).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_long_text_produces_multiple_chunks() {
        let tokenizer = test_tokenizer();
        let text = "word ".repeat(10000);
        let result = chunk_text(&text, 512, 128, &tokenizer).unwrap();
        assert!(result.len() >= 2);
    }

    #[test]
    fn test_overlap_clamping() {
        let tokenizer = test_tokenizer();
        let text = "word ".repeat(5000);
        let result = chunk_text(&text, 512, 500, &tokenizer).unwrap();
        assert!(result.len() >= 2);
    }
}
