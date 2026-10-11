use anyhow::{anyhow, Result};
use parking_lot::RwLock;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TokenizerJson {
    model: ModelConfig,
    added_tokens: Vec<AddedToken>,
}

#[derive(Debug, Deserialize)]
struct ModelConfig {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    model_type: String,
    unk_id: u32,
    vocab: Vec<(String, f32)>,
}

#[derive(Debug, Deserialize)]
struct AddedToken {
    id: u32,
    content: String,
    #[allow(dead_code)]
    special: bool,
}

/// SentencePiece Unigram tokenizer for E5 models
pub struct SentencePieceTokenizer {
    vocab: Arc<HashMap<String, u32>>,
    scores: Arc<HashMap<u32, f32>>,
    bos_id: u32,
    eos_id: u32,
    pad_id: u32,
    unk_id: u32,
    #[allow(dead_code)]
    max_length: usize,
    cache: Arc<RwLock<lru::LruCache<String, Vec<u32>>>>,
}

impl SentencePieceTokenizer {
    pub fn from_model_dir(model_dir: &Path) -> Result<Self> {
        let tokenizer_path = model_dir.join("tokenizer.json");
        let tokenizer_json = std::fs::read_to_string(&tokenizer_path)
            .map_err(|e| anyhow!("Failed to read tokenizer.json: {}", e))?;
        let data: TokenizerJson = serde_json::from_str(&tokenizer_json)
            .map_err(|e| anyhow!("Failed to parse tokenizer.json: {}", e))?;

        let mut vocab = HashMap::with_capacity(data.model.vocab.len());
        let mut scores = HashMap::with_capacity(data.model.vocab.len());

        for (idx, (token, score)) in data.model.vocab.iter().enumerate() {
            let id = idx as u32;
            vocab.insert(token.clone(), id);
            scores.insert(id, *score);
        }

        for token in &data.added_tokens {
            vocab.insert(token.content.clone(), token.id);
            scores.insert(token.id, 0.0);
        }

        let mut bos_id = 0u32;
        let mut eos_id = 2u32;
        let mut pad_id = 1u32;
        let mut unk_id = data.model.unk_id;

        for token in &data.added_tokens {
            match token.content.as_str() {
                "<s>" => bos_id = token.id,
                "</s>" => eos_id = token.id,
                "<pad>" => pad_id = token.id,
                "<unk>" => unk_id = token.id,
                _ => {}
            }
        }

        Ok(Self {
            vocab: Arc::new(vocab),
            scores: Arc::new(scores),
            bos_id,
            eos_id,
            pad_id,
            unk_id,
            max_length: 510,
            cache: Arc::new(RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(10000).unwrap(),
            ))),
        })
    }

    pub fn encode(&self, text: &str, add_special_tokens: bool) -> Result<Vec<u32>> {
        // Keyed by a hash of the whole text: two passages sharing a long
        // prefix (the contextual "Document: ... Section: ..." header) and a
        // length must never share token ids.
        let cache_key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hasher);
            add_special_tokens.hash(&mut hasher);
            format!("{}:{:016x}", text.len(), hasher.finish())
        };
        if let Some(cached) = self.cache.write().get(&cache_key) {
            return Ok(cached.clone());
        }

        let mut token_ids = Vec::new();

        if add_special_tokens {
            token_ids.push(self.bos_id);
        }

        let tokens = self.tokenize_unigram(text)?;
        token_ids.extend(tokens);

        if add_special_tokens {
            token_ids.push(self.eos_id);
        }

        if text.len() < 1000 {
            self.cache.write().put(cache_key, token_ids.clone());
        }

        Ok(token_ids)
    }

    /// Token id used for padding (`<pad>`).
    pub fn pad_id(&self) -> u32 {
        self.pad_id
    }

    /// Viterbi-based Unigram tokenization (SentencePiece algorithm)
    fn tokenize_unigram(&self, text: &str) -> Result<Vec<u32>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // Replace spaces with SentencePiece word boundary marker
        let processed = format!("▁{}", text.replace(' ', "▁"));
        // Byte offset of every char boundary, so candidate pieces are
        // borrowed slices of `processed` rather than freshly built Strings.
        let offsets: Vec<usize> = processed
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(processed.len()))
            .collect();
        let n = offsets.len() - 1;

        // Viterbi forward pass: find best segmentation
        let mut best_score = vec![f32::NEG_INFINITY; n + 1];
        let mut best_edge = vec![0usize; n + 1];
        best_score[0] = 0.0;

        for end in 1..=n {
            let max_piece_len = 32.min(end);
            for start in (end.saturating_sub(max_piece_len))..end {
                let piece = &processed[offsets[start]..offsets[end]];
                if let Some(&id) = self.vocab.get(piece) {
                    let score = best_score[start] + self.scores.get(&id).copied().unwrap_or(0.0);
                    if score > best_score[end] {
                        best_score[end] = score;
                        best_edge[end] = start;
                    }
                }
            }

            // Fallback: if no valid segmentation found, use single character as unknown
            if best_score[end] == f32::NEG_INFINITY && best_score[end - 1] > f32::NEG_INFINITY {
                best_score[end] = best_score[end - 1] - 10.0;
                best_edge[end] = end - 1;
            }
        }

        // Viterbi backward pass: reconstruct best path
        let mut tokens = Vec::new();
        let mut pos = n;
        while pos > 0 {
            let start = best_edge[pos];
            let piece = &processed[offsets[start]..offsets[pos]];
            let id = self.vocab.get(piece).copied().unwrap_or(self.unk_id);
            tokens.push(id);
            pos = start;
        }

        tokens.reverse();
        Ok(tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenizer() -> (tempfile::TempDir, SentencePieceTokenizer) {
        let dir = tempfile::tempdir().expect("tempdir");
        let json = r#"{
            "model": {"type": "Unigram", "unk_id": 3, "vocab": [
                ["<s>", 0.0], ["<pad>", 0.0], ["</s>", 0.0], ["<unk>", 0.0],
                ["▁", -2.0], ["a", -3.0], ["b", -3.0], ["▁a", -1.5], ["▁b", -1.5], ["ab", -1.0]
            ]},
            "added_tokens": [
                {"id": 0, "content": "<s>", "special": true},
                {"id": 1, "content": "<pad>", "special": true},
                {"id": 2, "content": "</s>", "special": true},
                {"id": 3, "content": "<unk>", "special": true}
            ]
        }"#;
        std::fs::write(dir.path().join("tokenizer.json"), json).expect("write tokenizer");
        let tok = SentencePieceTokenizer::from_model_dir(dir.path()).expect("load tokenizer");
        (dir, tok)
    }

    #[test]
    fn texts_sharing_a_long_prefix_and_length_are_tokenized_separately() {
        let (_dir, tok) = tokenizer();
        let prefix = "a ".repeat(80);
        let first = format!("{prefix}ab");
        let second = format!("{prefix}ba");
        assert_eq!(first.len(), second.len());
        let a = tok.encode(&first, true).expect("encode");
        let b = tok.encode(&second, true).expect("encode");
        assert_ne!(a, b);
        // Cached results stay correct.
        assert_eq!(tok.encode(&first, true).expect("encode"), a);
    }

    #[test]
    fn viterbi_prefers_higher_scoring_pieces_and_handles_unknown_chars() {
        let (_dir, tok) = tokenizer();
        // "▁ab": "▁" + "ab" scores -3.0, "▁a" + "b" scores -4.5.
        assert_eq!(tok.encode("ab", false).expect("encode"), vec![4, 9]);
        let with_unknown = tok.encode("aé", false).expect("encode");
        assert_eq!(with_unknown, vec![7, 3]);
    }
}
