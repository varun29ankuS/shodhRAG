use anyhow::{anyhow, Result};
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::Session;
use ort::value::Value;
use parking_lot::{Mutex, RwLock};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use super::tokenizer::SentencePieceTokenizer;
use super::EmbeddingModel;
use crate::compute::calibration::Calibration;
use crate::compute::provider::{self, Accelerator, ComputePreference, CpuReason, Device};
use crate::compute::speed;
use crate::compute::status::{ComputeStatus, EmbedderReport};

#[derive(Clone)]
pub struct E5Config {
    pub model_path: PathBuf,
    pub dimension: usize,
    pub max_length: usize,
    pub normalize: bool,
    /// Whether a graphics card may run the model.
    pub compute: ComputePreference,
    /// Where the model's speed is reported (the engine's status).
    pub status: Option<Arc<ComputeStatus>>,
    /// Where the calibrated speed is kept between loads; without it the
    /// speed is measured at every load.
    pub calibration_file: Option<PathBuf>,
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
            compute: ComputePreference::Auto,
            status: None,
            calibration_file: None,
        })
    }

    /// This config with `compute` as the compute preference.
    pub fn with_compute(mut self, compute: ComputePreference) -> Self {
        self.compute = compute;
        self
    }

    /// The model file's name ("model_O4.onnx").
    pub fn model_file(&self) -> String {
        self.model_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
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

/// Passages timed by the calibration, after [`CALIBRATION_WARMUP`] untimed
/// ones (a graphics card compiles its kernels on the first run).
const CALIBRATION_PASSAGES: usize = 12;
const CALIBRATION_WARMUP: usize = 2;

/// What one calibration passage says (about 220 tokens with its number):
/// ordinary prose, at the length of a typical indexed chunk.
const CALIBRATION_TEXT: &str = "The committee reviewed the annual maintenance report for the \
    building and its grounds. The roof was inspected in the spring and two sections of flashing \
    were replaced after the winter storms. The heating system was serviced in October; its \
    boiler is now eleven years old and the engineer recommends budgeting for a replacement within \
    three years. Water pressure on the upper floors dropped twice during the summer, which the \
    city traced to work on the main line. Residents asked for more bicycle parking, better \
    lighting in the courtyard and a clearer schedule for the laundry room. The committee agreed \
    to collect quotes for the lighting, to trial a booking sheet for the laundry, and to ask the \
    landlord about converting part of the storage room into a bicycle shelter before the next \
    meeting, which will be held in the community room on the first Tuesday of the month.";

pub struct E5Embeddings {
    session: Arc<Mutex<Session>>,
    tokenizer: Arc<SentencePieceTokenizer>,
    config: E5Config,
    cache: Arc<RwLock<lru::LruCache<String, Vec<f32>>>>,
    status: Option<Arc<ComputeStatus>>,
    accelerator: Accelerator,
}

/// A session over the model on `device` (a graphics card) or the CPU
/// (`None`). A GPU provider that does not register is an error.
fn build_session(
    model_bytes: &[u8],
    model_dir: &Path,
    device: Option<&Device>,
    threads: usize,
) -> std::result::Result<Session, String> {
    let builder = Session::builder()
        .map_err(|e| format!("session builder: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("optimization level: {e}"))?
        .with_intra_threads(threads)
        .map_err(|e| format!("intra threads: {e}"))?
        .with_inter_threads(1)
        .map_err(|e| format!("inter threads: {e}"))?;
    let builder = with_provider(builder, model_dir, device)?;
    builder
        // No memory pattern: activation buffers are freed after each batch
        // (DirectML requires it off as well).
        .with_memory_pattern(false)
        .map_err(|e| format!("memory pattern: {e}"))?
        .commit_from_memory(model_bytes)
        .map_err(|e| format!("loading the model: {e}"))
}

fn with_provider(
    builder: SessionBuilder,
    model_dir: &Path,
    device: Option<&Device>,
) -> std::result::Result<SessionBuilder, String> {
    match device.map(|d| d.accelerator) {
        // No arena: with it, indexing a few pages left the largest batch's
        // buffers (about a gigabyte) resident until the model was unloaded.
        None | Some(Accelerator::Cpu) => builder
            .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
            .map_err(|e| format!("CPU provider: {e}")),
        Some(Accelerator::DirectMl) => direct_ml(builder, device),
        Some(Accelerator::CoreMl) => core_ml(builder, model_dir),
    }
}

fn direct_ml(
    builder: SessionBuilder,
    device: Option<&Device>,
) -> std::result::Result<SessionBuilder, String> {
    // DirectML runs operators one at a time.
    let mut builder = builder
        .with_parallel_execution(false)
        .map_err(|e| format!("execution mode: {e}"))?;
    let index = device.and_then(|d| d.index).unwrap_or(0);
    crate::compute::ep::append_directml(&mut builder, index)
        .map_err(|e| format!("DirectML: {e}"))?;
    Ok(builder)
}

fn core_ml(
    builder: SessionBuilder,
    model_dir: &Path,
) -> std::result::Result<SessionBuilder, String> {
    // The compiled Core ML model is kept next to the ONNX file, so a later
    // load does not compile it again.
    let cache = model_dir.join("coreml-cache");
    if let Err(e) = std::fs::create_dir_all(&cache) {
        return Err(format!("Core ML cache folder: {e}"));
    }
    let mut builder = builder;
    crate::compute::ep::append_coreml(
        &mut builder,
        &[
            // The ML Program format runs inputs of any length; the older
            // neural network format needs fixed shapes.
            ("ModelFormat", "MLProgram".to_string()),
            ("MLComputeUnits", "ALL".to_string()),
            ("ModelCacheDirectory", cache.display().to_string()),
        ],
    )
    .map_err(|e| format!("Core ML: {e}"))?;
    Ok(builder)
}

/// Token ids of the calibration passages: [`CALIBRATION_WARMUP`] +
/// [`CALIBRATION_PASSAGES`] passages, numbered so none repeats.
fn calibration_batch(tokenizer: &SentencePieceTokenizer) -> Result<Vec<Vec<u32>>> {
    (0..CALIBRATION_WARMUP + CALIBRATION_PASSAGES)
        .map(|i| {
            let mut ids = tokenizer.encode(
                &format!("passage: Item {}. {CALIBRATION_TEXT}", i + 1),
                true,
            )?;
            ids.truncate(512);
            Ok(ids)
        })
        .collect()
}

/// Tokens a second `session` embeds the calibration passages at.
fn measure(
    session: &Mutex<Session>,
    pad_id: u32,
    passages: &[Vec<u32>],
) -> std::result::Result<f64, String> {
    let (warmup, timed) = passages.split_at(CALIBRATION_WARMUP.min(passages.len()));
    fn as_slices(ids: &[Vec<u32>]) -> Vec<&[u32]> {
        ids.iter().map(Vec::as_slice).collect()
    }
    run_batch(session, pad_id, false, &as_slices(warmup)).map_err(|e| format!("{e:#}"))?;
    let tokens: usize = timed.iter().map(Vec::len).sum();
    let started = Instant::now();
    run_batch(session, pad_id, false, &as_slices(timed)).map_err(|e| format!("{e:#}"))?;
    speed::rate(tokens, started.elapsed())
        .ok_or_else(|| "the calibration batch took no measurable time".to_string())
}

/// Run one padded batch of token sequences through `session` and pool.
fn run_batch(
    session: &Mutex<Session>,
    pad_id: u32,
    normalize: bool,
    all_token_ids: &[&[u32]],
) -> Result<Vec<Vec<f32>>> {
    let mut all_embeddings = Vec::with_capacity(all_token_ids.len());
    let max_len = all_token_ids.iter().map(|ids| ids.len()).max().unwrap_or(0);
    let padded_len = max_len.clamp(1, 512);
    let batch_size = all_token_ids.len();

    let pad_id = i64::from(pad_id);
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

    let mut session = session.lock();
    let outputs = session
        .run(inputs)
        .map_err(|e| anyhow!("Batch inference failed: {:?}", e))?;

    // Extract per-sample embeddings from batch output
    // Check for sentence_embedding first (already pooled), then fall back to last_hidden_state
    let has_sentence_embedding = outputs.iter().any(|(name, _)| name == "sentence_embedding");

    if has_sentence_embedding {
        if let Ok((shape, data)) = outputs["sentence_embedding"].try_extract_tensor::<f32>() {
            let hidden_dim = shape[1] as usize;
            for sample_idx in 0..batch_size {
                let offset = sample_idx * hidden_dim;
                let embedding = data[offset..offset + hidden_dim].to_vec();
                all_embeddings.push(normalize_vec(normalize, embedding));
            }
        }
    } else if let Ok((shape, data)) = outputs["last_hidden_state"].try_extract_tensor::<f32>() {
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
                for value in pooled.iter_mut().take(hidden_dim) {
                    *value /= mask_sum;
                }
            }

            all_embeddings.push(normalize_vec(normalize, pooled));
        }
    }

    Ok(all_embeddings)
}

fn normalize_vec(normalize: bool, mut vec: Vec<f32>) -> Vec<f32> {
    if normalize {
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-12 {
            for v in &mut vec {
                *v /= norm;
            }
        }
    }
    vec
}

/// A session that runs, what runs it, and its calibrated speed.
struct Loaded {
    session: Session,
    accelerator: Accelerator,
    device: Option<String>,
    cpu_reason: Option<CpuReason>,
    tokens_per_second: f64,
    calibrated_at: chrono::DateTime<chrono::Utc>,
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

        let tokenizer = SentencePieceTokenizer::from_model_dir(model_dir)?;
        let threads = crate::compute::cores::inference_threads();
        let loaded =
            Self::load_on_best_target(&config, &model_bytes, model_dir, threads, &tokenizer)?;
        drop(model_bytes);

        let model_file = config.model_file();
        tracing::info!(
            provider = loaded.accelerator.label(),
            device = loaded.device.as_deref().unwrap_or("-"),
            cpu_reason = ?loaded.cpu_reason,
            threads,
            model = %model_file,
            pages_per_minute = speed::pages_per_minute(loaded.tokens_per_second).round(),
            "Search model loaded"
        );
        if let Some(status) = &config.status {
            status.embedder_loaded(
                EmbedderReport {
                    accelerator: loaded.accelerator,
                    device: loaded.device.clone(),
                    cpu_reason: loaded.cpu_reason.clone(),
                    threads: (!loaded.accelerator.is_gpu()).then_some(threads),
                    model_file,
                    pages_per_minute: speed::pages_per_minute(loaded.tokens_per_second),
                    speed: speed::classify(loaded.tokens_per_second),
                    recent_pages_per_minute: None,
                    degraded: false,
                    calibrated_at: loaded.calibrated_at.to_rfc3339(),
                    loaded: true,
                },
                loaded.tokens_per_second,
            );
        }

        Ok(Self {
            session: Arc::new(Mutex::new(loaded.session)),
            tokenizer: Arc::new(tokenizer),
            status: config.status.clone(),
            accelerator: loaded.accelerator,
            config,
            cache: Arc::new(RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(1000).unwrap(),
            ))),
        })
    }

    /// The session on the best target and its speed. A graphics card that
    /// starts is timed against the CPU (once, then from the calibration
    /// file) and kept only when it is faster.
    fn load_on_best_target(
        config: &E5Config,
        model_bytes: &[u8],
        model_dir: &Path,
        threads: usize,
        tokenizer: &SentencePieceTokenizer,
    ) -> Result<Loaded> {
        let selection = provider::select(config.compute, provider::probe_gpus, |device| {
            build_session(model_bytes, model_dir, device, threads)
        })
        .map_err(|e| anyhow!("Failed to load model: {e}"))?;
        let model_file = config.model_file();
        let model_size = model_bytes.len() as u64;
        let device_name = selection.device.as_ref().map(|d| d.name.clone());
        let now = chrono::Utc::now();
        let saved = config
            .calibration_file
            .as_deref()
            .and_then(Calibration::load)
            .filter(|c| {
                c.matches(
                    &model_file,
                    model_size,
                    config.compute,
                    device_name.as_deref(),
                    threads,
                    now,
                )
            });
        let pad_id = tokenizer.pad_id();
        let session = Mutex::new(selection.session);
        let calibration = match saved {
            Some(saved) => saved,
            None => {
                let passages = calibration_batch(tokenizer)?;
                let first = measure(&session, pad_id, &passages)
                    .map_err(|e| anyhow!("Timing the search model failed: {e}"))?;
                let (gpu, cpu) = if selection.accelerator.is_gpu() {
                    // The CPU's speed, from a session that lives only for
                    // the measurement.
                    let cpu_session = build_session(model_bytes, model_dir, None, threads)
                        .map_err(|e| anyhow!("Failed to load model: {e}"))?;
                    let cpu = measure(&Mutex::new(cpu_session), pad_id, &passages)
                        .map_err(|e| anyhow!("Timing the search model failed: {e}"))?;
                    (Some(first), cpu)
                } else {
                    (None, first)
                };
                let calibration = Calibration {
                    model_file: model_file.clone(),
                    model_size,
                    preference: config.compute,
                    device: device_name.clone(),
                    gpu_tokens_per_second: gpu,
                    cpu_tokens_per_second: cpu,
                    threads,
                    measured_at: now,
                };
                if let Some(path) = &config.calibration_file {
                    if let Err(e) = calibration.save(path) {
                        tracing::warn!(error = %e, "The search model's speed was not saved");
                    }
                }
                calibration
            }
        };
        let session = session.into_inner();
        let cpu = calibration.cpu_tokens_per_second;
        match (selection.device, calibration.gpu_tokens_per_second) {
            (Some(device), Some(gpu)) if provider::keep_gpu(gpu, cpu) => Ok(Loaded {
                session,
                accelerator: selection.accelerator,
                device: Some(device.name),
                cpu_reason: None,
                tokens_per_second: gpu,
                calibrated_at: calibration.measured_at,
            }),
            (Some(device), gpu) => {
                drop(session);
                let gpu = gpu.unwrap_or(0.0);
                tracing::warn!(
                    device = %device.name,
                    gpu_tokens_per_second = gpu,
                    cpu_tokens_per_second = cpu,
                    "The graphics card is slower than the CPU for the search model; using the CPU"
                );
                let session = build_session(model_bytes, model_dir, None, threads)
                    .map_err(|e| anyhow!("Failed to load model: {e}"))?;
                Ok(Loaded {
                    session,
                    accelerator: Accelerator::Cpu,
                    device: None,
                    cpu_reason: Some(CpuReason::SlowerThanCpu {
                        device: device.name,
                        gpu_tokens_per_second: gpu,
                        cpu_tokens_per_second: cpu,
                    }),
                    tokens_per_second: cpu,
                    calibrated_at: calibration.measured_at,
                })
            }
            (None, _) => Ok(Loaded {
                session,
                accelerator: Accelerator::Cpu,
                device: None,
                cpu_reason: selection.cpu_reason,
                tokens_per_second: cpu,
                calibrated_at: calibration.measured_at,
            }),
        }
    }

    /// What runs the model.
    pub fn accelerator(&self) -> Accelerator {
        self.accelerator
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

    /// Run one padded batch of token sequences through the model and pool,
    /// reporting its speed to the status.
    fn embed_token_batch(&self, all_token_ids: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        let started = Instant::now();
        let embeddings = run_batch(
            &self.session,
            self.tokenizer.pad_id(),
            self.config.normalize,
            all_token_ids,
        )?;
        if let Some(status) = &self.status {
            let tokens = all_token_ids.iter().map(|ids| ids.len()).sum();
            status.embedded(tokens, started.elapsed());
        }
        Ok(embeddings)
    }
}

impl Drop for E5Embeddings {
    fn drop(&mut self) {
        if let Some(status) = &self.status {
            status.embedder_unloaded();
        }
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

    fn model_config() -> E5Config {
        let root = std::env::var_os("SHODH_TEST_MODELS")
            .expect("SHODH_TEST_MODELS must point at the models directory");
        E5Config::auto_detect(Path::new(&root))
            .expect("E5 model files")
            .with_compute(ComputePreference::Cpu)
    }

    /// Length-sorted, token-budgeted batching returns vectors in input order:
    /// each batched vector matches its own single embedding, and matches it
    /// better than any other text's.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn batched_embeddings_match_single_embeddings_in_input_order() {
        let model = E5Embeddings::new(model_config()).expect("load E5");
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

    /// The CPU path loads, calibrates, reports itself and saves its speed;
    /// a second load reads the speed instead of measuring it again.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn the_cpu_path_reports_its_provider_and_keeps_its_calibration() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("search-speed.json");
        let status = Arc::new(ComputeStatus::default());
        let mut config = model_config();
        config.status = Some(status.clone());
        config.calibration_file = Some(file.clone());
        let model = E5Embeddings::new(config.clone()).expect("load E5");
        assert_eq!(model.accelerator(), Accelerator::Cpu);
        let report = status.embedder().expect("reported");
        assert!(report.loaded);
        assert_eq!(report.cpu_reason, Some(CpuReason::Chosen));
        assert!(report.pages_per_minute > 0.0);
        assert_eq!(
            report.threads,
            Some(crate::compute::cores::inference_threads())
        );
        let saved = Calibration::load(&file).expect("calibration saved");
        assert_eq!(saved.gpu_tokens_per_second, None);
        drop(model);
        assert!(status.embedder().is_some_and(|r| !r.loaded));

        let again = E5Embeddings::new(config).expect("load E5 again");
        let reloaded = status.embedder().expect("reported");
        assert_eq!(
            reloaded.calibrated_at, report.calibrated_at,
            "not measured again"
        );
        let vector = again.embed_query("lease notice period").expect("embedded");
        assert_eq!(vector.len(), 768);
    }

    /// When this machine has a graphics card the provider starts on, its
    /// vectors match the CPU's: the same text, the same direction within a
    /// small tolerance. CI's runners have none, and there is nothing to
    /// compare.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn a_graphics_card_embeds_like_the_cpu() {
        let cpu = E5Embeddings::new(model_config()).expect("load E5 on the CPU");
        let gpu =
            E5Embeddings::new(model_config().with_compute(ComputePreference::Auto)).expect("load");
        if !gpu.accelerator().is_gpu() {
            return;
        }
        let texts = [
            "The heating system was serviced in October.",
            "Negative eigenvalues let linear RNNs track parity.",
        ];
        let on_cpu = cpu
            .embed_batch_with_mode(&texts, E5Mode::Passage)
            .expect("cpu");
        let on_gpu = gpu
            .embed_batch_with_mode(&texts, E5Mode::Passage)
            .expect("gpu");
        for (a, b) in on_cpu.iter().zip(&on_gpu) {
            let cosine: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            assert!(cosine > 0.99, "the GPU's vector differs: cos={cosine}");
        }
    }
}
