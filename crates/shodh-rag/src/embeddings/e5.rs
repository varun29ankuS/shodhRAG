use anyhow::{anyhow, Result};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Value;
use parking_lot::{Mutex, RwLock};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::tokenizer::SentencePieceTokenizer;
use super::EmbeddingModel;

#[derive(Clone)]
pub struct E5Config {
    pub model_path: PathBuf,
    pub dimension: usize,
    pub max_length: usize,
    pub normalize: bool,
}

impl E5Config {
    pub fn auto_detect(model_dir: &Path) -> Option<Self> {
        let base_path = if model_dir.join("multilingual-e5-base").exists() {
            model_dir.join("multilingual-e5-base")
        } else if model_dir.join("multilingual-e5-large-instruct").exists() {
            model_dir.join("multilingual-e5-large-instruct")
        } else {
            return None;
        };

        let (dimension, model_file) = if base_path.to_string_lossy().contains("e5-base") {
            let quantized = base_path.join("model_qint8_avx512_vnni.onnx");
            let optimized = base_path.join("model_O4.onnx");
            if quantized.exists() {
                (768, "model_qint8_avx512_vnni.onnx")
            } else if optimized.exists() {
                (768, "model_O4.onnx")
            } else {
                (768, "model.onnx")
            }
        } else {
            (1024, "model.onnx")
        };

        let model_path = base_path.join(model_file);
        if !model_path.exists() {
            return None;
        }

        Some(Self {
            model_path,
            dimension,
            max_length: 512,
            normalize: true,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum E5Mode {
    Query,
    Passage,
}

/// Most sequences per inference batch.
const MAX_BATCH_ITEMS: usize = 64;
/// Most padded tokens per inference batch (sequences x longest length).
const MAX_BATCH_TOKENS: usize = 8192;

pub struct E5Embeddings {
    session: Arc<Mutex<Session>>,
    tokenizer: Arc<SentencePieceTokenizer>,
    config: E5Config,
    cache: Arc<RwLock<lru::LruCache<String, Vec<f32>>>>,
}

impl E5Embeddings {
    pub fn new(config: E5Config) -> Result<Self> {
        ort::init().with_name("e5_embeddings").commit();

        if !config.model_path.exists() {
            return Err(anyhow!(
                "Model file not found at: {}",
                config.model_path.display()
            ));
        }

        let model_bytes = std::fs::read(&config.model_path)
            .map_err(|e| anyhow!("Failed to read model: {:?}", e))?;

        let model_dir = config
            .model_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid model path"))?;

        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        // Model is loaded from memory (commit_from_memory), so no CWD change needed.
        let session = Session::builder()
            .map_err(|e| anyhow!("Session builder: {:?}", e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("Optimization level: {:?}", e))?
            .with_intra_threads(num_threads)
            .map_err(|e| anyhow!("Intra threads: {:?}", e))?
            .with_inter_threads(1)
            .map_err(|e| anyhow!("Inter threads: {:?}", e))?
            .with_memory_pattern(true)
            .map_err(|e| anyhow!("Memory pattern: {:?}", e))?
            .commit_from_memory(&model_bytes)
            .map_err(|e| anyhow!("Failed to load model: {:?}", e))?;

        let tokenizer = SentencePieceTokenizer::from_model_dir(model_dir)?;

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            tokenizer: Arc::new(tokenizer),
            config,
            cache: Arc::new(RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(1000).unwrap(),
            ))),
        })
    }

    pub fn embed_with_mode(&self, text: &str, mode: E5Mode) -> Result<Vec<f32>> {
        let prefixed = match mode {
            E5Mode::Query => format!("query: {}", text),
            E5Mode::Passage => format!("passage: {}", text),
        };

        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let cache_key = format!("{:?}:{:x}", mode, hasher.finish());
        if let Some(cached) = self.cache.write().get(&cache_key) {
            return Ok(cached.clone());
        }

        // The sequence is embedded unpadded, like passages (which are batched
        // with texts of similar length): the int8 model quantizes
        // activations per batch, so padding changes the vector.
        let mut token_ids = self.tokenizer.encode(&prefixed, true)?;
        token_ids.truncate(self.config.max_length.min(512));
        let embedding = self
            .embed_token_batch(&[token_ids.as_slice()])?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("embedding model returned no vector"))?;

        self.cache.write().put(cache_key, embedding.clone());
        Ok(embedding)
    }

    fn normalize_vec(&self, mut vec: Vec<f32>) -> Result<Vec<f32>> {
        if self.config.normalize {
            let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 1e-12 {
                for v in &mut vec {
                    *v /= norm;
                }
            }
        }
        Ok(vec)
    }

    pub fn embed_batch_with_mode(&self, texts: &[&str], mode: E5Mode) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Tokenize everything first, then batch texts of similar length:
        // a batch is padded to its longest member, so mixing a 20-token
        // bibliography entry with a 500-token section wastes most of the
        // compute. Batches are capped by padded token volume, not count.
        let mut tokenized: Vec<(usize, Vec<u32>)> = Vec::with_capacity(texts.len());
        for (index, text) in texts.iter().enumerate() {
            let prefixed = match mode {
                E5Mode::Query => format!("query: {}", text),
                E5Mode::Passage => format!("passage: {}", text),
            };
            let mut token_ids = self.tokenizer.encode(&prefixed, true)?;
            token_ids.truncate(512);
            tokenized.push((index, token_ids));
        }
        tokenized.sort_by_key(|(_, ids)| ids.len());

        let mut results: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut start = 0;
        while start < tokenized.len() {
            let mut end = start;
            while end < tokenized.len() {
                let candidate = end + 1 - start;
                let padded = tokenized[end].1.len().max(1) * candidate;
                if candidate > MAX_BATCH_ITEMS || (padded > MAX_BATCH_TOKENS && end > start) {
                    break;
                }
                end += 1;
            }
            let batch: Vec<&[u32]> = tokenized[start..end]
                .iter()
                .map(|(_, ids)| ids.as_slice())
                .collect();
            let embeddings = self.embed_token_batch(&batch)?;
            if embeddings.len() != batch.len() {
                return Err(anyhow!(
                    "embedding model returned {} vectors for {} inputs",
                    embeddings.len(),
                    batch.len()
                ));
            }
            for ((index, _), embedding) in tokenized[start..end].iter().zip(embeddings) {
                results[*index] = Some(embedding);
            }
            start = end;
        }
        results
            .into_iter()
            .map(|r| r.ok_or_else(|| anyhow!("missing embedding for a batch input")))
            .collect()
    }

    /// Run one padded batch of token sequences through the model and pool.
    fn embed_token_batch(&self, all_token_ids: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        let mut all_embeddings = Vec::with_capacity(all_token_ids.len());
        {
            let max_len = all_token_ids.iter().map(|ids| ids.len()).max().unwrap_or(0);
            let padded_len = max_len.clamp(1, 512);
            let batch_size = all_token_ids.len();

            let pad_id = i64::from(self.tokenizer.pad_id());
            let mut input_ids_flat = Vec::with_capacity(batch_size * padded_len);
            let mut attention_mask_flat = Vec::with_capacity(batch_size * padded_len);

            for token_ids in all_token_ids {
                for &id in token_ids.iter() {
                    input_ids_flat.push(id as i64);
                    attention_mask_flat.push(1i64);
                }
                for _ in token_ids.len()..padded_len {
                    input_ids_flat.push(pad_id);
                    attention_mask_flat.push(0i64);
                }
            }

            let shape = vec![batch_size, padded_len];
            let input_ids = Value::from_array((shape.clone(), input_ids_flat))
                .map_err(|e| anyhow!("input_ids tensor: {:?}", e))?;
            let attention_mask = Value::from_array((shape, attention_mask_flat.clone()))
                .map_err(|e| anyhow!("attention_mask tensor: {:?}", e))?;

            let inputs = ort::inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
            ];

            let mut session = self.session.lock();
            let outputs = session
                .run(inputs)
                .map_err(|e| anyhow!("Batch inference failed: {:?}", e))?;

            // Extract per-sample embeddings from batch output
            // Check for sentence_embedding first (already pooled), then fall back to last_hidden_state
            let has_sentence_embedding =
                outputs.iter().any(|(name, _)| name == "sentence_embedding");

            if has_sentence_embedding {
                if let Ok((shape, data)) = outputs["sentence_embedding"].try_extract_tensor::<f32>()
                {
                    let hidden_dim = shape[1] as usize;
                    for sample_idx in 0..batch_size {
                        let offset = sample_idx * hidden_dim;
                        let embedding = data[offset..offset + hidden_dim].to_vec();
                        all_embeddings.push(self.normalize_vec(embedding)?);
                    }
                }
            } else if let Ok((shape, data)) =
                outputs["last_hidden_state"].try_extract_tensor::<f32>()
            {
                let seq_len = shape[1] as usize;
                let hidden_dim = shape[2] as usize;

                for sample_idx in 0..batch_size {
                    let mask_offset = sample_idx * padded_len;
                    let sample_offset = sample_idx * seq_len * hidden_dim;
                    let mut pooled = vec![0.0f32; hidden_dim];
                    let mut mask_sum = 0.0f32;

                    for pos in 0..seq_len {
                        let mask_val = if mask_offset + pos < attention_mask_flat.len() {
                            attention_mask_flat[mask_offset + pos] as f32
                        } else {
                            0.0
                        };
                        if mask_val > 0.0 {
                            mask_sum += mask_val;
                            let offset = sample_offset + pos * hidden_dim;
                            for dim in 0..hidden_dim {
                                pooled[dim] += data[offset + dim] * mask_val;
                            }
                        }
                    }

                    if mask_sum > 0.0 {
                        for dim in 0..hidden_dim {
                            pooled[dim] /= mask_sum;
                        }
                    }

                    all_embeddings.push(self.normalize_vec(pooled)?);
                }
            }
        }

        Ok(all_embeddings)
    }
}

impl EmbeddingModel for E5Embeddings {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_with_mode(text, E5Mode::Query)
    }

    fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_with_mode(text, E5Mode::Passage)
    }

    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed_batch_with_mode(texts, E5Mode::Passage)
    }

    fn dimension(&self) -> usize {
        self.config.dimension
    }

    fn count_tokens(&self, text: &str) -> Option<usize> {
        self.tokenizer
            .encode(&format!("passage: {}", text), true)
            .ok()
            .map(|ids| ids.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Length-sorted, token-budgeted batching returns vectors in input order:
    /// each batched vector matches its own single embedding, and matches it
    /// better than any other text's.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn batched_embeddings_match_single_embeddings_in_input_order() {
        let root = std::env::var_os("SHODH_TEST_MODELS")
            .expect("SHODH_TEST_MODELS must point at the models directory");
        let config = E5Config::auto_detect(Path::new(&root)).expect("E5 model files");
        let model = E5Embeddings::new(config).expect("load E5");
        let long = "Linear attention replaces softmax with a kernel feature map. ".repeat(30);
        let texts = [
            long.as_str(),
            "[12] S. Hochreiter and J. Schmidhuber. Long short-term memory. 1997.",
            "Negative eigenvalues let linear RNNs track parity.",
            "The delta rule updates the state with a rank-one correction.",
        ];
        let batched = model
            .embed_batch_with_mode(&texts, E5Mode::Passage)
            .expect("batch");
        assert_eq!(batched.len(), texts.len());
        let mut singles: Vec<Vec<f32>> = Vec::new();
        for (text, vector) in texts.iter().zip(&batched) {
            let single = model
                .embed_batch_with_mode(&[*text], E5Mode::Passage)
                .expect("single")
                .remove(0);
            let cosine: f32 = single.iter().zip(vector).map(|(a, b)| a * b).sum();
            singles.push(single);
            // The int8 model quantizes activations per batch, so batch
            // composition moves vectors slightly; never by this much.
            assert!(
                cosine > 0.97,
                "batched vector differs for {text:?}: cos={cosine}"
            );
        }
        for (i, vector) in batched.iter().enumerate() {
            let best = singles
                .iter()
                .enumerate()
                .map(|(j, s)| (j, s.iter().zip(vector).map(|(a, b)| a * b).sum::<f32>()))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(j, _)| j);
            assert_eq!(best, Some(i), "vector {i} is closest to another text");
        }
    }
}
