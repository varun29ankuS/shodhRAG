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
        let type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&t| t as i64).collect();

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

#[cfg(test)]
mod tests {
    use super::*;

    const MODELS_ENV: &str = "SHODH_TEST_MODELS";

    /// Resolve `$SHODH_TEST_MODELS/<sub>`. The models are not checked in, so these
    /// tests are `#[ignore]`d unless `SHODH_TEST_MODELS` was set at build time (see
    /// build.rs). Once enabled -- or when run with `--ignored` -- a missing variable
    /// or missing file is a hard failure, never a silent skip.
    fn model_dir(sub: &str) -> PathBuf {
        let root = std::env::var_os(MODELS_ENV).unwrap_or_else(|| {
            panic!("{MODELS_ENV} must point at the models directory to run this test")
        });
        let dir = PathBuf::from(root).join(sub);
        let tokenizer = dir.join("tokenizer.json");
        assert!(
            tokenizer.is_file(),
            "expected tokenizer at {}",
            tokenizer.display()
        );
        dir
    }

    /// MiniLM WordPiece tokenizer.json must load with the minimal `tokenizers`
    /// feature set (no `esaxx_fast`, no `progressbar`) and produce BERT pair framing.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing ms-marco-MiniLM-L6-v2/"
    )]
    fn minilm_wordpiece_tokenizer_loads_and_frames_pairs() {
        let dir = model_dir("ms-marco-MiniLM-L6-v2");
        let tok = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .expect("load MiniLM tokenizer.json");
        let cls = tok.token_to_id("[CLS]").expect("[CLS] in vocab");
        let sep = tok.token_to_id("[SEP]").expect("[SEP] in vocab");

        let enc = tok
            .encode(
                ("what is rust", "Rust is a systems programming language"),
                true,
            )
            .expect("encode pair");
        let ids = enc.get_ids();
        assert_eq!(ids.first(), Some(&cls));
        assert_eq!(ids.last(), Some(&sep));
        assert_eq!(ids.iter().filter(|&&id| id == sep).count(), 2);
        assert!(enc.get_type_ids().contains(&0));
        assert!(enc.get_type_ids().contains(&1));
        assert!(enc.get_attention_mask().iter().all(|&m| m == 1));
    }

    /// E5 (XLM-R SentencePiece Unigram) tokenizer.json must also load and encode
    /// through `tokenizers` without the C++ esaxx backend, and agree on framing
    /// with the in-crate SentencePiece tokenizer used for embeddings.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn e5_unigram_tokenizer_loads_and_frames_sequences() {
        let dir = model_dir("multilingual-e5-base");
        let tok = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .expect("load E5 tokenizer.json");
        let bos = tok.token_to_id("<s>").expect("<s> in vocab");
        let eos = tok.token_to_id("</s>").expect("</s> in vocab");

        let enc = tok
            .encode("query: नमस्ते दुनिया, hello world", true)
            .expect("encode");
        let ids = enc.get_ids();
        assert!(ids.len() > 2, "expected content tokens, got {ids:?}");
        assert_eq!(ids.first(), Some(&bos));
        assert_eq!(ids.last(), Some(&eos));

        let native = crate::embeddings::tokenizer::SentencePieceTokenizer::from_model_dir(&dir)
            .expect("load in-crate SentencePiece tokenizer");
        let native_ids = native
            .encode("query: hello world", true)
            .expect("native encode");
        assert_eq!(native_ids.first(), Some(&bos));
        assert_eq!(native_ids.last(), Some(&eos));
    }

    /// End-to-end: tokenizer + ONNX session. A relevant passage must outscore an
    /// unrelated one for the same query.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing ms-marco-MiniLM-L6-v2/ with an ONNX model"
    )]
    fn cross_encoder_ranks_relevant_passage_higher() {
        let dir = model_dir("ms-marco-MiniLM-L6-v2");
        let reranker = CrossEncoderReranker::new(&dir).expect("load reranker");
        let query = "how do vaccines train the immune system";
        let relevant = "Vaccines expose the immune system to an antigen so it learns to produce antibodies against the pathogen.";
        let unrelated = "The Eiffel Tower was completed in 1889 for the World's Fair in Paris.";

        let s_rel = reranker.score(query, relevant).expect("score relevant");
        let s_unrel = reranker.score(query, unrelated).expect("score unrelated");
        assert!(
            s_rel > s_unrel,
            "relevant {s_rel} should outscore unrelated {s_unrel}"
        );

        let ranked = reranker
            .rerank_batch(
                query,
                &[
                    ("unrelated".to_string(), unrelated.to_string()),
                    ("relevant".to_string(), relevant.to_string()),
                ],
                2,
            )
            .expect("batch rerank");
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, "relevant");
    }
}
