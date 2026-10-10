//! Natural-language inference (entailment) cross-encoder for checking that
//! a passage supports a claim.
//!
//! The model is `cross-encoder/nli-deberta-v3-xsmall` (quantised ONNX export,
//! pinned in [`crate::embeddings::model_store::answer_check_artifacts`]). It
//! reads a (premise, hypothesis) pair and returns logits for contradiction,
//! entailment and neutral; the label order is read from the model's
//! `config.json` and checked at load time, so a different export cannot
//! silently swap them.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Value;
use parking_lot::Mutex;

/// The ONNX file of the pinned export.
pub const NLI_MODEL_FILE: &str = "model_quint8_avx2.onnx";
const MAX_LENGTH: usize = 512;
const MAX_BATCH: usize = 8;

/// Probabilities of one (premise, hypothesis) pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entailment {
    pub contradiction: f32,
    pub entailment: f32,
    pub neutral: f32,
}

/// The entailment model. Cloning is cheap and shares the loaded session.
#[derive(Clone)]
pub struct NliModel {
    session: Arc<Mutex<Session>>,
    tokenizer: Arc<tokenizers::Tokenizer>,
    /// Logit index of contradiction, entailment and neutral.
    labels: [usize; 3],
    pad_id: i64,
}

impl std::fmt::Debug for NliModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NliModel")
            .field("labels", &self.labels)
            .finish_non_exhaustive()
    }
}

/// Logit indexes of contradiction, entailment and neutral from a model
/// config's `id2label`.
fn label_indexes(config: &serde_json::Value) -> Result<[usize; 3]> {
    let id2label = config
        .get("id2label")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("config.json has no id2label"))?;
    let mut found = [None; 3];
    for (id, label) in id2label {
        let index: usize = id
            .parse()
            .with_context(|| format!("label id {id} is not a number"))?;
        let slot = match label.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("contradiction") => 0,
            Some("entailment") => 1,
            Some("neutral") => 2,
            other => return Err(anyhow!("unexpected NLI label {other:?}")),
        };
        found[slot] = Some(index);
    }
    match found {
        [Some(c), Some(e), Some(n)] if c < 3 && e < 3 && n < 3 => Ok([c, e, n]),
        _ => Err(anyhow!(
            "config.json must label exactly contradiction, entailment and neutral"
        )),
    }
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|e| e / sum).collect()
}

impl NliModel {
    /// Load the model from `dir` (`model_quint8_avx2.onnx`, `tokenizer.json`,
    /// `config.json`).
    pub fn new(dir: &Path) -> Result<Self> {
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("config.json"))
                .with_context(|| format!("reading {}", dir.join("config.json").display()))?,
        )
        .context("config.json is not valid JSON")?;
        let labels = label_indexes(&config)?;
        let tokenizer = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow!("Failed to load the NLI tokenizer: {e}"))?;
        let pad_id = tokenizer.token_to_id("[PAD]").map(i64::from).unwrap_or(0);
        let bytes = std::fs::read(dir.join(NLI_MODEL_FILE))
            .with_context(|| format!("reading {}", dir.join(NLI_MODEL_FILE).display()))?;
        let session = Session::builder()
            .map_err(|e| anyhow!("Session builder: {e:?}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("Opt level: {e:?}"))?
            .commit_from_memory(&bytes)
            .map_err(|e| anyhow!("Failed to load the NLI model: {e:?}"))?;
        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            tokenizer: Arc::new(tokenizer),
            labels,
            pad_id,
        })
    }

    /// Classify (premise, hypothesis) pairs. Returns one result per pair, in
    /// order.
    pub fn classify(&self, pairs: &[(&str, &str)]) -> Result<Vec<Entailment>> {
        let mut out = Vec::with_capacity(pairs.len());
        for chunk in pairs.chunks(MAX_BATCH) {
            let encodings = chunk
                .iter()
                .map(|(premise, hypothesis)| {
                    self.tokenizer
                        .encode((*premise, *hypothesis), true)
                        .map_err(|e| anyhow!("NLI tokenization failed: {e}"))
                })
                .collect::<Result<Vec<_>>>()?;
            let width = encodings
                .iter()
                .map(|e| e.get_ids().len().min(MAX_LENGTH))
                .max()
                .unwrap_or(1)
                .max(1);
            let mut ids = Vec::with_capacity(chunk.len() * width);
            let mut mask = Vec::with_capacity(chunk.len() * width);
            for enc in &encodings {
                let len = enc.get_ids().len().min(width);
                ids.extend(enc.get_ids()[..len].iter().map(|&id| i64::from(id)));
                mask.extend(
                    enc.get_attention_mask()[..len]
                        .iter()
                        .map(|&m| i64::from(m)),
                );
                ids.extend(std::iter::repeat_n(self.pad_id, width - len));
                mask.extend(std::iter::repeat_n(0_i64, width - len));
            }
            let shape = vec![chunk.len(), width];
            let input_ids = Value::from_array((shape.clone(), ids))
                .map_err(|e| anyhow!("NLI input_ids: {e:?}"))?;
            let attention_mask = Value::from_array((shape, mask))
                .map_err(|e| anyhow!("NLI attention_mask: {e:?}"))?;
            let inputs = ort::inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
            ];
            let mut session = self.session.lock();
            let outputs = session
                .run(inputs)
                .map_err(|e| anyhow!("NLI inference failed: {e:?}"))?;
            let (_shape, data) = outputs["logits"]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("Failed to extract NLI logits: {e:?}"))?;
            if data.len() != chunk.len() * 3 {
                return Err(anyhow!(
                    "NLI model returned {} logits for {} pairs",
                    data.len(),
                    chunk.len()
                ));
            }
            for row in data.chunks(3) {
                let p = softmax(row);
                out.push(Entailment {
                    contradiction: p[self.labels[0]],
                    entailment: p[self.labels[1]],
                    neutral: p[self.labels[2]],
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_come_from_the_config() {
        let config = json!({"id2label": {"0": "contradiction", "1": "entailment", "2": "neutral"}});
        assert_eq!(label_indexes(&config).unwrap(), [0, 1, 2]);
        let swapped =
            json!({"id2label": {"0": "entailment", "1": "neutral", "2": "contradiction"}});
        assert_eq!(label_indexes(&swapped).unwrap(), [2, 0, 1]);
        assert!(label_indexes(&json!({"id2label": {"0": "LABEL_0", "1": "LABEL_1"}})).is_err());
        assert!(label_indexes(&json!({})).is_err());
    }

    #[test]
    fn softmax_sums_to_one() {
        let p = softmax(&[2.0, 0.5, -1.0]);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(p[0] > p[1] && p[1] > p[2]);
    }

    /// End-to-end with the pinned model: an entailed hypothesis beats a
    /// contradicted one.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing nli-deberta-v3-xsmall/"
    )]
    fn entailment_beats_contradiction_with_the_pinned_model() {
        let root = std::env::var_os("SHODH_TEST_MODELS")
            .expect("SHODH_TEST_MODELS must point at the models directory");
        let model = NliModel::new(&std::path::PathBuf::from(root).join("nli-deberta-v3-xsmall"))
            .expect("load the NLI model");
        let premise = "Either party may terminate the agreement with sixty days written notice.";
        let out = model
            .classify(&[
                (
                    premise,
                    "The agreement can be terminated with 60 days notice.",
                ),
                (premise, "The agreement cannot be terminated."),
            ])
            .expect("classify");
        assert!(out[0].entailment > 0.5, "{out:?}");
        assert!(out[1].contradiction > out[1].entailment, "{out:?}");
    }
}
