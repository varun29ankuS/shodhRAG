use anyhow::{anyhow, Result};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Value;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Cross-encoder reranker using ms-marco-MiniLM-L6-v2
pub struct CrossEncoderReranker {
    session: Arc<Mutex<Session>>,
    tokenizer: Arc<tokenizers::Tokenizer>,
    max_length: usize,
}

impl CrossEncoderReranker {
    pub fn new(model_dir: &Path) -> Result<Self> {
        let model_path = Self::find_model(model_dir)?;
        let tokenizer_path = model_dir.join("tokenizer.json");

        if !tokenizer_path.exists() {
            return Err(anyhow!(
                "Tokenizer not found at: {}",
                tokenizer_path.display()
            ));
        }

        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow!("Failed to load tokenizer: {:?}", e))?;

        let model_bytes = std::fs::read(&model_path)?;
        let session = Session::builder()
            .map_err(|e| anyhow!("Session builder: {:?}", e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("Opt level: {:?}", e))?
            .commit_from_memory(&model_bytes)
            .map_err(|e| anyhow!("Failed to load reranker model: {:?}", e))?;

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            tokenizer: Arc::new(tokenizer),
            max_length: 512,
        })
    }

    fn find_model(model_dir: &Path) -> Result<PathBuf> {
        let candidates = [
            model_dir.join("model_O4.onnx"),
            model_dir.join("model.onnx"),
        ];
        for path in &candidates {
            if path.exists() {
                return Ok(path.clone());
            }
        }
        Err(anyhow!(
            "No reranker model found in: {}",
            model_dir.display()
        ))
    }

    /// Score a (query, document) pair. Higher score = more relevant.
    pub fn score(&self, query: &str, document: &str) -> Result<f32> {
        let encoding = self
            .tokenizer
            .encode((query, document), true)
            .map_err(|e| anyhow!("Tokenization failed: {:?}", e))?;

        let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&m| m as i64)
            .collect();
        let type_ids: Vec<i64> = encoding
            .get_type_ids()
            .iter()
            .map(|&t| t as i64)
            .collect();

        let len = ids.len().min(self.max_length);
        let ids = &ids[..len];
        let mask = &mask[..len];
        let type_ids = &type_ids[..len];

        let shape = vec![1, len];

        let input_ids = Value::from_array((shape.clone(), ids.to_vec()))
            .map_err(|e| anyhow!("input_ids: {:?}", e))?;
        let attention_mask = Value::from_array((shape.clone(), mask.to_vec()))
            .map_err(|e| anyhow!("attention_mask: {:?}", e))?;
        let token_type_ids = Value::from_array((shape, type_ids.to_vec()))
            .map_err(|e| anyhow!("token_type_ids: {:?}", e))?;

        let inputs = ort::inputs![
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
            "token_type_ids" => token_type_ids,
        ];

        let mut session = self.session.lock();
        let outputs = session
            .run(inputs)
            .map_err(|e| anyhow!("Reranker inference failed: {:?}", e))?;

        let (_shape, data) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("Failed to extract logits: {:?}", e))?;

        if data.is_empty() {
            return Err(anyhow!("Cross-encoder returned empty logits tensor"));
        }
        Ok(data[0])
    }

    /// Rerank a list of (id, text) pairs by relevance to query.
    /// Returns (id, reranked_score) sorted descending.
    pub fn rerank(
        &self,
        query: &str,
        candidates: &[(String, String)],
        top_k: usize,
    ) -> Result<Vec<(String, f32)>> {
        self.rerank_batch(query, candidates, top_k)
    }

    /// Batch reranking — tokenize all (query, doc) pairs and run ONNX inference
    /// in batches of MAX_BATCH for better throughput.
    pub fn rerank_batch(
        &self,
        query: &str,
        candidates: &[(String, String)],
        top_k: usize,
    ) -> Result<Vec<(String, f32)>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        const MAX_BATCH: usize = 16;
        let mut all_scored: Vec<(String, f32)> = Vec::with_capacity(candidates.len());

        for chunk in candidates.chunks(MAX_BATCH) {
            // Pair each encoding with its original candidate to maintain alignment
            // when tokenization fails for some candidates
            let paired: Vec<(&(String, String), _)> = chunk
                .iter()
                .filter_map(|candidate| {
                    self.tokenizer
                        .encode((query, candidate.1.as_str()), true)
                        .ok()
                        .map(|enc| (candidate, enc))
                })
                .collect();

            if paired.is_empty() {
                continue;
            }

            let encodings: Vec<_> = paired.iter().map(|(_, enc)| enc).collect();

            let max_len = encodings
                .iter()
                .map(|e| e.get_ids().len().min(self.max_length))
                .max()
                .unwrap_or(128);
            let batch_size = encodings.len();

            let mut ids_flat = Vec::with_capacity(batch_size * max_len);
            let mut mask_flat = Vec::with_capacity(batch_size * max_len);
            let mut type_flat = Vec::with_capacity(batch_size * max_len);

            for enc in &encodings {
                let len = enc.get_ids().len().min(max_len);
                for i in 0..len {
                    ids_flat.push(enc.get_ids()[i] as i64);
                    mask_flat.push(enc.get_attention_mask()[i] as i64);
                    type_flat.push(enc.get_type_ids()[i] as i64);
                }
                // Pad to max_len
                for _ in len..max_len {
                    ids_flat.push(0i64);
                    mask_flat.push(0i64);
                    type_flat.push(0i64);
                }
            }

            let shape = vec![batch_size, max_len];
            let input_ids = Value::from_array((shape.clone(), ids_flat))
                .map_err(|e| anyhow!("batch input_ids: {:?}", e))?;
            let attention_mask = Value::from_array((shape.clone(), mask_flat))
                .map_err(|e| anyhow!("batch attention_mask: {:?}", e))?;
            let token_type_ids = Value::from_array((shape, type_flat))
                .map_err(|e| anyhow!("batch token_type_ids: {:?}", e))?;

            let inputs = ort::inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
                "token_type_ids" => token_type_ids,
            ];

            let mut session = self.session.lock();
            let outputs = session
                .run(inputs)
                .map_err(|e| anyhow!("Batch reranker inference failed: {:?}", e))?;

            // logits shape: [batch_size, 1] — one score per candidate
            let output_key = outputs
                .iter()
                .next()
                .map(|(name, _)| name.to_string())
                .unwrap_or_else(|| "logits".to_string());
            let (_shape, data) = outputs[output_key.as_str()]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("Failed to extract batch logits: {:?}", e))?;

            if data.len() != paired.len() {
                tracing::warn!(
                    "Cross-encoder output count {} != candidate count {}, some candidates may be unscored",
                    data.len(), paired.len()
                );
            }
            for (i, (candidate, _)) in paired.iter().enumerate() {
                if i < data.len() {
                    all_scored.push((candidate.0.clone(), data[i]));
                }
            }
        }

        all_scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        all_scored.truncate(top_k);
        Ok(all_scored)
    }
}
