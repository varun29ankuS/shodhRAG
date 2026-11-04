use anyhow::{anyhow, Result};
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::Session;
use ort::value::Value;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::tokenizer::SentencePieceTokenizer;
use super::EmbeddingModel;
use crate::compute::calibration::{CalibrationKey, Calibrations};
use crate::compute::choose::{self, Candidate, Learned};
use crate::compute::provider::{self, Accelerator, ComputePreference, CpuReason, Device};
use crate::compute::speed;
use crate::compute::status::{ComputeStatus, EmbedderReport};

/// The folder (under the model root) holding the E5 model's files.
pub const E5_DIR: &str = "multilingual-e5-base";
/// The dimension of multilingual-e5-base's vectors (either export).
pub const E5_DIMENSION: usize = 768;

/// The exports of multilingual-e5-base Shodh runs. Their vectors are close
/// but not the same, so an index holds vectors of one export only
/// ([`crate::embeddings::index_model`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum E5Variant {
    /// `model_O4.onnx` (optimum O4: fp16 weights), 555 MB. Runs on a
    /// graphics card or the CPU, with sequences batched.
    #[default]
    Fp16,
    /// `model_qint8_avx512_vnni.onnx` (int8 weights, activations quantised
    /// while it runs), 279 MB: about 3x faster than fp16 on the CPU. The CPU
    /// only, and each sequence is embedded on its own ([`Self::batches`]).
    Int8,
}

impl E5Variant {
    pub const ALL: [E5Variant; 2] = [E5Variant::Fp16, E5Variant::Int8];

    /// The export's file name in [`E5_DIR`].
    pub fn file_name(self) -> &'static str {
        match self {
            E5Variant::Fp16 => "model_O4.onnx",
            E5Variant::Int8 => "model_qint8_avx512_vnni.onnx",
        }
    }

    /// "fp16" / "int8", as records and reports name it.
    pub fn label(self) -> &'static str {
        match self {
            E5Variant::Fp16 => "fp16",
            E5Variant::Int8 => "int8",
        }
    }

    /// Whether a graphics card may run it. The int8 export's integer
    /// operators are a CPU design (DirectML and Core ML were never measured
    /// on it), so it stays on the CPU.
    pub fn runs_on_gpu(self) -> bool {
        matches!(self, E5Variant::Fp16)
    }

    /// Whether several sequences may share one inference batch. The int8
    /// export quantises activations with `DynamicQuantizeLinear`, whose
    /// scale is taken over the whole input tensor: in a batch, a text's
    /// vector would depend on the other texts and on the padding. Embedded
    /// alone (no other rows, no padding), its vector depends only on the
    /// text.
    pub fn batches(self) -> bool {
        matches!(self, E5Variant::Fp16)
    }

    /// The variant whose file is named `file`.
    pub fn of_file(file: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.file_name() == file)
    }
}

/// How this processor runs the int8 export's integer matrix products
/// (ONNX Runtime's MLAS kernels for unsigned-by-signed 8-bit products).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Int8Kernels {
    /// x86 with VNNI (AVX512-VNNI or AVX-VNNI): exact 32-bit sums.
    Vnni,
    /// ARM64 (dot-product or widening multiplies): exact 32-bit sums.
    Arm,
    /// Any other x86 (AVX2 without VNNI, as on AMD Zen 3 and Intel before
    /// Ice Lake / Alder Lake): `vpmaddubsw` sums pairs into 16 bits and
    /// clamps them (ONNX Runtime's documented U8S8 saturation), so the int8
    /// export's vectors come out different. On CI the int8 memory-use scores
    /// of one fixture ran about 0.03 higher on one runner while fp16 stayed
    /// identical; two runners with AVX512-VNNI gave the same scores. Not used
    /// for new int8 indexes.
    Saturating,
}

/// The int8 kernels of this processor.
pub fn int8_kernels() -> Int8Kernels {
    #[cfg(target_arch = "x86_64")]
    {
        // AVX-VNNI is CPUID leaf 7, sub-leaf 1, EAX bit 4; it needs the AVX
        // state, which AVX2 being usable implies.
        #[allow(unused_unsafe)]
        let avx_vnni = std::arch::is_x86_feature_detected!("avx2")
            && unsafe { std::arch::x86_64::__cpuid_count(7, 1) }.eax & (1 << 4) != 0;
        if std::arch::is_x86_feature_detected!("avx512vnni") || avx_vnni {
            Int8Kernels::Vnni
        } else {
            Int8Kernels::Saturating
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        Int8Kernels::Arm
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        Int8Kernels::Saturating
    }
}

/// Whether this processor runs the int8 export exactly, so int8 ("faster
/// search, slightly less accurate") may be offered. Elsewhere only fp16 is.
pub fn int8_exact() -> bool {
    int8_kernels() != Int8Kernels::Saturating
}

/// Which export the vectors must be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantRule {
    /// Vectors of this export exist (an index built with it): it is the
    /// one used, on whatever runs it (fp16: a graphics card or the CPU;
    /// int8: the CPU).
    Index(E5Variant),
    /// No vectors exist yet: the library is built with this export, the
    /// user's choice for a new library (fp16 unless they opted in to int8).
    /// int8 is used only where it is installed and runs exactly
    /// ([`int8_exact`]); otherwise fp16. The export loaded is the library's
    /// ([`E5Embeddings::variant`]).
    New(E5Variant),
}

#[derive(Clone)]
pub struct E5Config {
    /// The [`E5_DIR`] folder: its tokenizer and the installed exports.
    pub model_dir: PathBuf,
    pub rule: VariantRule,
    pub dimension: usize,
    pub max_length: usize,
    pub normalize: bool,
    /// Whether a graphics card may run the model.
    pub compute: ComputePreference,
    /// Where the model's speed is reported (the engine's status).
    pub status: Option<Arc<ComputeStatus>>,
    /// Where measured speeds and verdicts are kept between loads
    /// ([`crate::compute::calibration`]); without it they are measured at
    /// every load.
    pub calibration_file: Option<PathBuf>,
}

/// The exports installed in `dir` (an [`E5_DIR`] folder with its tokenizer).
pub fn installed_variants(dir: &Path) -> Vec<E5Variant> {
    if !dir.join("tokenizer.json").exists() {
        return Vec::new();
    }
    E5Variant::ALL
        .into_iter()
        .filter(|v| dir.join(v.file_name()).exists())
        .collect()
}

impl E5Config {
    /// The E5 model under the model root `model_dir` when its tokenizer and
    /// at least one export are installed, with the default rule for a new
    /// library, fp16 (the engine sets the rule its index needs).
    pub fn auto_detect(model_dir: &Path) -> Option<Self> {
        let dir = model_dir.join(E5_DIR);
        if installed_variants(&dir).is_empty() {
            return None;
        }
        Some(Self {
            model_dir: dir,
            rule: VariantRule::New(E5Variant::Fp16),
            dimension: E5_DIMENSION,
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

    /// This config with `rule` as the variant rule.
    pub fn with_rule(mut self, rule: VariantRule) -> Self {
        self.rule = rule;
        self
    }

    /// Whether the files `rule` needs are installed.
    pub fn can_load(&self) -> bool {
        let installed = installed_variants(&self.model_dir);
        match self.rule {
            VariantRule::Index(variant) => installed.contains(&variant),
            VariantRule::New(_) => !installed.is_empty(),
        }
    }

    fn path_of(&self, variant: E5Variant) -> PathBuf {
        self.model_dir.join(variant.file_name())
    }

    /// What may run on a graphics card and what on the CPU under the rule,
    /// given the installed exports and this processor's int8 kernels.
    fn plan(&self) -> Result<Plan> {
        self.plan_for(int8_exact())
    }

    /// [`Self::plan`] on a processor that runs int8 exactly (`int8_exact`)
    /// or not.
    fn plan_for(&self, int8_exact: bool) -> Result<Plan> {
        let installed = installed_variants(&self.model_dir);
        match self.rule {
            VariantRule::Index(variant) => {
                if !installed.contains(&variant) {
                    return Err(anyhow!(
                        "the search model this library was indexed with ({}, {}) is not installed in {}",
                        variant.label(),
                        variant.file_name(),
                        self.model_dir.display()
                    ));
                }
                Ok(Plan {
                    gpu: variant.runs_on_gpu().then_some(variant),
                    cpu: variant,
                })
            }
            VariantRule::New(chosen) => {
                let has = |v: E5Variant| installed.contains(&v);
                // The chosen export when it can run here (int8: installed and
                // exact); else fp16; with only int8 installed (a library moved
                // to another computer before it had vectors), int8 rather
                // than nothing.
                let usable = |v: E5Variant| has(v) && (v == E5Variant::Fp16 || int8_exact);
                let variant = if usable(chosen) {
                    chosen
                } else if has(E5Variant::Fp16) {
                    E5Variant::Fp16
                } else if has(E5Variant::Int8) {
                    E5Variant::Int8
                } else {
                    return Err(anyhow!(
                        "no E5 search model is installed in {}",
                        self.model_dir.display()
                    ));
                };
                if variant != chosen {
                    tracing::warn!(
                        chosen = chosen.label(),
                        used = variant.label(),
                        "The search model chosen for a new library cannot run here; using another"
                    );
                }
                Ok(Plan {
                    gpu: variant.runs_on_gpu().then_some(variant),
                    cpu: variant,
                })
            }
        }
    }
}

/// The export each target runs: `gpu` on a graphics card (none: the CPU
/// only), `cpu` on the processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Plan {
    gpu: Option<E5Variant>,
    cpu: E5Variant,
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

/// Passages of the calibration batch. It is run once untimed at the same
/// shape (a graphics card compiles its kernels for each shape on its first
/// run), then [`CALIBRATION_RUNS`] times timed; the median is the speed.
const CALIBRATION_PASSAGES: usize = 6;
const CALIBRATION_RUNS: usize = 3;

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

/// The session that embeds and what runs it.
struct Active {
    session: Session,
    on_gpu: bool,
    accelerator: Accelerator,
    device: Option<Device>,
}

pub struct E5Embeddings {
    active: Mutex<Active>,
    /// Calls embedding now (one runs, the others wait for the session).
    in_flight: AtomicUsize,
    tokenizer: Arc<SentencePieceTokenizer>,
    config: E5Config,
    /// The export every vector of this load is in; a CPU that takes over
    /// from a failed graphics card runs the same one.
    variant: E5Variant,
    model_path: PathBuf,
    cache: Arc<RwLock<lru::LruCache<String, Vec<f32>>>>,
    status: Option<Arc<ComputeStatus>>,
    /// This load's generation in the status.
    generation: u64,
    threads: usize,
    key: CalibrationKey,
}

/// A session over the model at `model_path` on `device` (a graphics card)
/// or the CPU (`None`). A GPU provider that does not register is an error.
/// `core_ml_cache` is where Core ML keeps its compiled model.
fn build_session(
    model_path: &Path,
    core_ml_cache: Option<&Path>,
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
    let builder = with_provider(builder, core_ml_cache, device)?;
    builder
        // No memory pattern: activation buffers are freed after each batch
        // (DirectML requires it off as well).
        .with_memory_pattern(false)
        .map_err(|e| format!("memory pattern: {e}"))?
        // From the file: no copy of the model is held while sessions are
        // built and timed.
        .commit_from_file(model_path)
        .map_err(|e| format!("loading the model: {e}"))
}

fn with_provider(
    builder: SessionBuilder,
    core_ml_cache: Option<&Path>,
    device: Option<&Device>,
) -> std::result::Result<SessionBuilder, String> {
    match device.map(|d| d.accelerator) {
        // No arena: with it, indexing a few pages left the largest batch's
        // buffers (about a gigabyte) resident until the model was unloaded.
        None | Some(Accelerator::Cpu) => builder
            .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
            .map_err(|e| format!("CPU provider: {e}")),
        Some(Accelerator::DirectMl) => direct_ml(builder, device),
        Some(Accelerator::CoreMl) => match core_ml_cache {
            Some(cache) => core_ml(builder, cache),
            None => Err("Core ML: no folder for its compiled model".to_string()),
        },
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

fn core_ml(builder: SessionBuilder, cache: &Path) -> std::result::Result<SessionBuilder, String> {
    if let Err(e) = std::fs::create_dir_all(cache) {
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

/// Where Core ML keeps the model it compiled from the file at `model_path`:
/// a folder named by the file's size and modification time, so a new model
/// file (same graph, new weights) is compiled again; folders of earlier
/// files are removed. Core ML itself recompiles only when the graph changes.
fn core_ml_cache(model_path: &Path) -> PathBuf {
    let root = model_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("coreml-cache");
    let stamp = std::fs::metadata(model_path)
        .map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            format!("{}-{modified}", m.len())
        })
        .unwrap_or_else(|_| "unknown".to_string());
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy() != stamp {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    root.join(stamp)
}

/// Token ids of the calibration passages, numbered so none repeats.
fn calibration_batch(tokenizer: &SentencePieceTokenizer) -> Result<Vec<Vec<u32>>> {
    (0..CALIBRATION_PASSAGES)
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

fn as_slices(ids: &[Vec<u32>]) -> Vec<&[u32]> {
    ids.iter().map(Vec::as_slice).collect()
}

/// Tokens a second `session` (of `variant`) embeds `passages` at, run the
/// way that export always runs ([`run_sequences`]): one untimed run at the
/// same shape, then the median of [`CALIBRATION_RUNS`] timed runs.
fn measure(
    session: &mut Session,
    variant: E5Variant,
    pad_id: u32,
    passages: &[Vec<u32>],
) -> std::result::Result<f64, String> {
    let batch = as_slices(passages);
    let tokens: usize = passages.iter().map(Vec::len).sum();
    run_sequences(session, variant, pad_id, false, &batch).map_err(|e| format!("{e:#}"))?;
    let mut rates = Vec::with_capacity(CALIBRATION_RUNS);
    for _ in 0..CALIBRATION_RUNS {
        let (_, elapsed) =
            run_sequences(session, variant, pad_id, false, &batch).map_err(|e| format!("{e:#}"))?;
        rates.push(
            speed::rate(tokens, elapsed)
                .ok_or_else(|| "the calibration batch took no measurable time".to_string())?,
        );
    }
    rates.sort_by(f64::total_cmp);
    Ok(rates[rates.len() / 2])
}

/// A session and the export it runs.
struct Built {
    session: Session,
    variant: E5Variant,
}

/// Builds and times sessions of the model for [`choose::choose`]: a
/// graphics card runs the plan's `gpu` export, the CPU its `cpu` export.
struct E5Candidate<'a> {
    config: &'a E5Config,
    plan: Plan,
    core_ml_cache: Option<&'a Path>,
    threads: usize,
    pad_id: u32,
    passages: &'a [Vec<u32>],
}

impl Candidate for E5Candidate<'_> {
    type Session = Built;
    fn build(&mut self, device: Option<&Device>) -> std::result::Result<Built, String> {
        let variant = match device {
            Some(_) => self
                .plan
                .gpu
                .ok_or_else(|| "no search model for a graphics card is installed".to_string())?,
            None => self.plan.cpu,
        };
        let session = build_session(
            &self.config.path_of(variant),
            self.core_ml_cache,
            device,
            self.threads,
        )?;
        Ok(Built { session, variant })
    }
    fn time(&mut self, built: &mut Built) -> std::result::Result<f64, String> {
        measure(
            &mut built.session,
            built.variant,
            self.pad_id,
            self.passages,
        )
    }
}

/// Embeds token sequences with `session` the way `variant` must run: in one
/// padded batch, or (int8, [`E5Variant::batches`]) each sequence on its own,
/// so a text's vector never depends on the texts it was sent with. The time
/// is the inference's alone.
fn run_sequences(
    session: &mut Session,
    variant: E5Variant,
    pad_id: u32,
    normalize: bool,
    all_token_ids: &[&[u32]],
) -> Result<(Vec<Vec<f32>>, Duration)> {
    if variant.batches() || all_token_ids.len() <= 1 {
        return run_batch(session, pad_id, normalize, all_token_ids);
    }
    let mut vectors = Vec::with_capacity(all_token_ids.len());
    let mut elapsed = Duration::ZERO;
    for ids in all_token_ids {
        let (mut one, took) = run_batch(session, pad_id, normalize, std::slice::from_ref(ids))?;
        vectors.append(&mut one);
        elapsed += took;
    }
    Ok((vectors, elapsed))
}

/// Run one padded batch of token sequences through `session` and pool.
/// The time is the inference's alone.
fn run_batch(
    session: &mut Session,
    pad_id: u32,
    normalize: bool,
    all_token_ids: &[&[u32]],
) -> Result<(Vec<Vec<f32>>, Duration)> {
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

    let started = Instant::now();
    let outputs = session
        .run(inputs)
        .map_err(|e| anyhow!("Batch inference failed: {:?}", e))?;
    let elapsed = started.elapsed();

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

    Ok((all_embeddings, elapsed))
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

impl E5Embeddings {
    pub fn new(config: E5Config) -> Result<Self> {
        ort::init().with_name("e5_embeddings").commit();

        let plan = config.plan()?;
        // The calibration holds for the files the targets run: one export,
        // or (no index yet) the graphics card's and the CPU's together, since
        // a card's verdict is a comparison with the CPU's export.
        let files: Vec<E5Variant> = match plan.gpu {
            Some(gpu) if gpu != plan.cpu => vec![gpu, plan.cpu],
            _ => vec![plan.cpu],
        };
        let mut model_size = 0;
        for variant in &files {
            model_size += std::fs::metadata(config.path_of(*variant))
                .map_err(|e| {
                    anyhow!(
                        "Failed to read the search model {}: {e}",
                        variant.file_name()
                    )
                })?
                .len();
        }
        let key_file = files
            .iter()
            .map(|v| v.file_name())
            .collect::<Vec<_>>()
            .join("+");

        let tokenizer = SentencePieceTokenizer::from_model_dir(&config.model_dir)?;
        let threads = crate::compute::cores::inference_threads();
        let key = CalibrationKey::new(&key_file, model_size, threads);
        let mut saved = config
            .calibration_file
            .as_deref()
            .map(Calibrations::load)
            .unwrap_or_default();
        let now = chrono::Utc::now();
        let passages = calibration_batch(&tokenizer)?;
        let core_ml_cache = plan.gpu.map(|v| core_ml_cache(&config.path_of(v)));
        let mut candidate = E5Candidate {
            config: &config,
            plan,
            core_ml_cache: core_ml_cache.as_deref(),
            threads,
            pad_id: tokenizer.pad_id(),
            passages: &passages,
        };
        // An export no graphics card runs: the CPU, for that reason when the
        // setting would have allowed a card.
        let (preference, cpu_model) = match plan.gpu {
            Some(_) => (config.compute, false),
            None => (
                ComputePreference::Cpu,
                config.compute == ComputePreference::Auto,
            ),
        };
        let mut chosen = choose::choose(
            &mut candidate,
            preference,
            provider::probe_gpus,
            &|device: Option<&Device>| saved.known(&key, device, now),
        )
        .map_err(|e| anyhow!("Failed to load model: {e}"))?;
        if cpu_model {
            chosen.cpu_reason = Some(CpuReason::CpuModel);
        }
        let variant = chosen.session.variant;
        let model_file = variant.file_name().to_string();
        if variant == E5Variant::Int8 && !int8_exact() {
            // An int8 index made elsewhere (or the only export installed):
            // queries stay in its export, but this processor's kernels round
            // differently from the ones that built it.
            tracing::warn!(
                kernels = ?int8_kernels(),
                "The int8 search model runs on a processor without VNNI; its vectors differ slightly from an exact processor's"
            );
        }

        // A speed measured while OCR competed for the cores is used for this
        // load but not kept; a card's failure is kept all the same.
        let competing = crate::compute::contention::ocr_active();
        let keep: Vec<Learned> = chosen
            .learned
            .iter()
            .filter(|l| {
                !competing
                    || matches!(
                        l,
                        Learned::Rejected {
                            reason: CpuReason::Failed { .. },
                            ..
                        }
                    )
            })
            .cloned()
            .collect();
        let calibrated_at = if chosen.tokens_per_second.is_some()
            && chosen.learned.iter().any(|l| match (l, &chosen.device) {
                (Learned::Kept { .. }, Some(_)) => true,
                (Learned::Cpu { .. }, None) => true,
                _ => false,
            }) {
            Some(now)
        } else {
            saved.measured_at(&key, chosen.device.as_ref(), now)
        };
        if !keep.is_empty() {
            saved.learn(&key, &keep, now);
            if let Some(path) = &config.calibration_file {
                if let Err(e) = saved.save(path) {
                    tracing::warn!(error = %e, "The search model's speed was not saved");
                }
            }
        }

        tracing::info!(
            provider = chosen.accelerator.label(),
            device = chosen.device.as_ref().map_or("-", |d| d.name.as_str()),
            cpu_reason = ?chosen.cpu_reason,
            threads,
            model = %model_file,
            variant = variant.label(),
            pages_per_minute = chosen.tokens_per_second.map(|t| speed::pages_per_minute(t).round()),
            "Search model loaded"
        );
        let generation = match &config.status {
            Some(status) => status.embedder_loaded(
                EmbedderReport {
                    accelerator: chosen.accelerator,
                    device: chosen.device.as_ref().map(|d| d.name.clone()),
                    cpu_reason: chosen.cpu_reason.clone(),
                    threads: (!chosen.accelerator.is_gpu()).then_some(threads),
                    model_file,
                    pages_per_minute: chosen.tokens_per_second.map(speed::pages_per_minute),
                    speed: chosen.tokens_per_second.map(speed::classify),
                    recent_pages_per_minute: None,
                    degraded: false,
                    calibrated_at: calibrated_at.map(|t| t.to_rfc3339()),
                    loaded: true,
                },
                chosen.tokens_per_second,
            ),
            None => 0,
        };

        let model_path = config.path_of(variant);
        Ok(Self {
            active: Mutex::new(Active {
                session: chosen.session.session,
                on_gpu: chosen.accelerator.is_gpu(),
                accelerator: chosen.accelerator,
                device: chosen.device,
            }),
            in_flight: AtomicUsize::new(0),
            tokenizer: Arc::new(tokenizer),
            status: config.status.clone(),
            config,
            variant,
            model_path,
            cache: Arc::new(RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(1000).unwrap(),
            ))),
            generation,
            threads,
            key,
        })
    }

    /// What runs the model now.
    pub fn accelerator(&self) -> Accelerator {
        self.active.lock().accelerator
    }

    /// The export every vector of this load is in.
    pub fn variant(&self) -> E5Variant {
        self.variant
    }

    /// A graphics card failed while embedding and the CPU took over: kept
    /// in the calibration (the card is not tried again until its driver,
    /// the app or the setting changes) and shown in the status.
    fn moved_to_cpu(&self, device: Option<Device>, error: String) {
        let reason = CpuReason::Failed {
            device: device.as_ref().map(|d| d.name.clone()),
            error,
        };
        if let (Some(path), Some(device)) = (&self.config.calibration_file, device) {
            let now = chrono::Utc::now();
            let mut saved = Calibrations::load(path);
            saved.learn(
                &self.key,
                &[Learned::Rejected {
                    device,
                    reason: reason.clone(),
                }],
                now,
            );
            if let Err(e) = saved.save(path) {
                tracing::warn!(error = %e, "The graphics card's failure was not saved");
            }
        }
        if let Some(status) = &self.status {
            status.embedder_moved(self.generation, Accelerator::Cpu, reason, self.threads);
        }
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

        // Alone and unpadded: the same vector the text gets inside a batch
        // of passages for the int8 export (each sequence runs alone there).
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
    /// reporting its speed to the status. A graphics card that fails here
    /// hands over to the CPU, and the batch is embedded there.
    fn embed_token_batch(&self, all_token_ids: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        let tokens: usize = all_token_ids.iter().map(|ids| ids.len()).sum();
        // Another call running or waiting: this batch's speed says nothing
        // about the machine.
        let waited = self.in_flight.fetch_add(1, Ordering::SeqCst) > 0;
        let ocr_before = crate::compute::contention::ocr_active();
        let result = {
            let mut active = self.active.lock();
            let Active {
                session, on_gpu, ..
            } = &mut *active;
            let ran = choose::run_or_fall_back(
                session,
                on_gpu,
                |s| {
                    run_sequences(
                        s,
                        self.variant,
                        self.tokenizer.pad_id(),
                        self.config.normalize,
                        all_token_ids,
                    )
                    .map_err(|e| format!("{e:#}"))
                },
                // The same export on the CPU: the vectors stay in it.
                || build_session(&self.model_path, None, None, self.threads),
            );
            match ran {
                Ok((value, Some(error))) => {
                    let device = active.device.take();
                    active.accelerator = Accelerator::Cpu;
                    drop(active);
                    self.moved_to_cpu(device, error);
                    Ok((value, None))
                }
                Ok((value, None)) => Ok((value, Some(()))),
                Err(e) => Err(e),
            }
        };
        let contended = waited
            || self.in_flight.load(Ordering::SeqCst) > 1
            || ocr_before
            || crate::compute::contention::ocr_active();
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let ((embeddings, elapsed), timed) = result.map_err(|e| anyhow!(e))?;
        if let (Some(status), Some(())) = (&self.status, timed) {
            status.embedded(tokens, elapsed, contended);
        }
        Ok(embeddings)
    }
}

impl Drop for E5Embeddings {
    fn drop(&mut self) {
        if let Some(status) = &self.status {
            status.embedder_unloaded(self.generation);
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

    fn e5_variant(&self) -> Option<E5Variant> {
        Some(self.variant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The installed E5 model on the CPU, with no index yet (fp16, the default
    /// for a new library).
    fn model_config() -> E5Config {
        let root = std::env::var_os("SHODH_TEST_MODELS")
            .expect("SHODH_TEST_MODELS must point at the models directory");
        E5Config::auto_detect(Path::new(&root))
            .expect("E5 model files")
            .with_compute(ComputePreference::Cpu)
    }

    /// `variant` on the CPU, as for an index built with it. CI installs both.
    fn variant_config(variant: E5Variant) -> E5Config {
        let config = model_config().with_rule(VariantRule::Index(variant));
        assert!(
            config.can_load(),
            "{} is not installed under SHODH_TEST_MODELS",
            variant.file_name()
        );
        config
    }

    #[test]
    fn the_variants_name_their_files_and_how_they_run() {
        assert_eq!(E5Variant::Fp16.file_name(), "model_O4.onnx");
        assert_eq!(E5Variant::Int8.file_name(), "model_qint8_avx512_vnni.onnx");
        for variant in E5Variant::ALL {
            assert_eq!(E5Variant::of_file(variant.file_name()), Some(variant));
        }
        assert_eq!(E5Variant::of_file("model.onnx"), None);
        assert!(E5Variant::Fp16.batches() && E5Variant::Fp16.runs_on_gpu());
        assert!(!E5Variant::Int8.batches() && !E5Variant::Int8.runs_on_gpu());
        assert_eq!(serde_json::to_value(E5Variant::Int8).unwrap(), "int8");
    }

    /// Which export runs where: an index's export everywhere it can run; a
    /// new library fp16 unless int8 was chosen, and int8 only where it is
    /// installed and exact.
    #[test]
    fn the_plan_follows_the_index_and_the_choice_for_a_new_library() {
        let dir = tempfile::tempdir().expect("temp dir");
        let e5 = dir.path().join(E5_DIR);
        std::fs::create_dir_all(&e5).unwrap();
        assert!(
            E5Config::auto_detect(dir.path()).is_none(),
            "nothing installed"
        );
        std::fs::write(e5.join("tokenizer.json"), b"{}").unwrap();
        assert!(E5Config::auto_detect(dir.path()).is_none(), "no export");
        std::fs::write(e5.join(E5Variant::Fp16.file_name()), b"fp16").unwrap();
        let config = E5Config::auto_detect(dir.path()).expect("fp16 installed");
        assert_eq!(
            config.rule,
            VariantRule::New(E5Variant::Fp16),
            "fp16 by default"
        );
        let plan = |rule| config.clone().with_rule(rule).plan_for(true);
        let plan_saturating = |rule| config.clone().with_rule(rule).plan_for(false);
        let fp16 = Plan {
            gpu: Some(E5Variant::Fp16),
            cpu: E5Variant::Fp16,
        };
        let int8 = Plan {
            gpu: None,
            cpu: E5Variant::Int8,
        };
        assert_eq!(plan(VariantRule::New(E5Variant::Fp16)).unwrap(), fp16);
        assert_eq!(
            plan(VariantRule::New(E5Variant::Int8)).unwrap(),
            fp16,
            "int8 chosen but not installed: fp16"
        );
        assert!(plan(VariantRule::Index(E5Variant::Int8)).is_err());
        assert!(!config
            .clone()
            .with_rule(VariantRule::Index(E5Variant::Int8))
            .can_load());

        std::fs::write(e5.join(E5Variant::Int8.file_name()), b"int8").unwrap();
        assert_eq!(
            installed_variants(&e5),
            vec![E5Variant::Fp16, E5Variant::Int8]
        );
        assert_eq!(
            plan(VariantRule::New(E5Variant::Fp16)).unwrap(),
            fp16,
            "int8 installed is not used unless chosen"
        );
        assert_eq!(plan(VariantRule::New(E5Variant::Int8)).unwrap(), int8);
        assert_eq!(
            plan_saturating(VariantRule::New(E5Variant::Int8)).unwrap(),
            fp16,
            "a processor without exact int8 kernels builds fp16"
        );
        assert_eq!(
            plan_saturating(VariantRule::Index(E5Variant::Int8)).unwrap(),
            int8,
            "an int8 index stays int8 everywhere"
        );
        assert_eq!(
            plan(VariantRule::Index(E5Variant::Fp16)).unwrap(),
            fp16,
            "an fp16 index stays fp16 on the CPU"
        );
        assert_eq!(plan(VariantRule::Index(E5Variant::Int8)).unwrap(), int8);

        std::fs::remove_file(e5.join(E5Variant::Fp16.file_name())).unwrap();
        assert_eq!(
            plan_saturating(VariantRule::New(E5Variant::Fp16)).unwrap(),
            int8,
            "only int8 installed"
        );
        assert!(plan(VariantRule::Index(E5Variant::Fp16)).is_err());
    }

    /// The int8 export quantises activations over its whole input, so its
    /// vectors would move with their batch; each sequence runs alone
    /// instead. A text's vector is the same, bit for bit, alone, in batches
    /// of other texts (shorter, longer, more than one inference batch, any
    /// order), as a single document and after the model is loaded again.
    /// Bit-identical holds on one machine with one thread count (the same
    /// kernels on the same shapes); another processor may round differently,
    /// as with any export.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn an_int8_vector_does_not_depend_on_the_texts_it_is_embedded_with() {
        let model = E5Embeddings::new(variant_config(E5Variant::Int8)).expect("load int8");
        assert_eq!(model.variant(), E5Variant::Int8);
        let target = "Negative eigenvalues let linear RNNs track parity.";
        let alone = model
            .embed_batch_with_mode(&[target], E5Mode::Passage)
            .expect("alone")
            .remove(0);
        // Which kernels ran, and a digest of the vector: equal digests on
        // runners of one processor class, different ones across classes.
        let digest = alone.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| {
            (h ^ u64::from(x.to_bits())).wrapping_mul(0x0000_0100_0000_01b3)
        });
        eprintln!(
            "processor {:?}; int8 kernels {:?}; vector digest {digest:016x}",
            std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default(),
            int8_kernels()
        );
        let long = "Linear attention replaces softmax with a kernel feature map. ".repeat(30);
        let many: Vec<String> = (0..80)
            .map(|i| {
                format!("Item {i}: the delta rule updates the state with a rank-one correction.")
            })
            .collect();
        let mut crowded: Vec<&str> = many.iter().map(String::as_str).collect();
        crowded.insert(37, target);
        let batches: Vec<Vec<&str>> = vec![
            vec![long.as_str(), target],
            vec![target, "[12] Long short-term memory. 1997.", long.as_str()],
            vec![
                "Short.",
                target,
                "A medium sentence about state space models and gating.",
            ],
            crowded,
        ];
        let same_bits = |a: &[f32], b: &[f32]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
        };
        for batch in &batches {
            let index = batch.iter().position(|t| *t == target).unwrap();
            let vectors = model
                .embed_batch_with_mode(batch, E5Mode::Passage)
                .expect("batch");
            assert_eq!(vectors.len(), batch.len());
            let max_diff = vectors[index]
                .iter()
                .zip(&alone)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            assert!(
                same_bits(&vectors[index], &alone),
                "in a batch of {} the vector moved (largest difference {max_diff:e})",
                batch.len()
            );
        }
        let document = model.embed_document(target).expect("document");
        assert!(same_bits(&document, &alone), "embedded as one document");
        drop(model);
        let again = E5Embeddings::new(variant_config(E5Variant::Int8)).expect("load again");
        let reloaded = again
            .embed_batch_with_mode(&[long.as_str(), target], E5Mode::Passage)
            .expect("reloaded");
        assert!(same_bits(&reloaded[1], &alone), "after loading again");
    }

    /// Length-sorted, token-budgeted batching returns vectors in input order:
    /// each batched vector matches its own single embedding, and matches it
    /// better than any other text's. For both exports.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn batched_embeddings_match_single_embeddings_in_input_order() {
        for variant in E5Variant::ALL {
            batched_embeddings_match_single_embeddings(variant);
        }
    }

    fn batched_embeddings_match_single_embeddings(variant: E5Variant) {
        let model = E5Embeddings::new(variant_config(variant)).expect("load E5");
        assert_eq!(model.variant(), variant);
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
            // Batch composition moves vectors slightly (padding, kernels);
            // never by this much.
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

    /// The CPU path loads, measures, reports itself and saves its speed; a
    /// second load reads the speed instead of measuring it again, and an
    /// unload marks the report not loaded.
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
        // No index yet: fp16, the default for a new library, even with int8
        // installed.
        let expected = E5Variant::Fp16;
        assert_eq!(model.variant(), expected);
        let report = status.embedder().expect("reported");
        assert_eq!(report.model_file, expected.file_name());
        assert!(report.loaded);
        assert_eq!(report.cpu_reason, Some(CpuReason::Chosen));
        assert!(report.pages_per_minute.is_some_and(|p| p > 0.0));
        assert!(report.calibrated_at.is_some());
        assert_eq!(
            report.threads,
            Some(crate::compute::cores::inference_threads())
        );
        let saved = Calibrations::load(&file);
        assert_eq!(saved.entries.len(), 1, "{saved:?}");
        drop(model);
        assert!(status.embedder().is_some_and(|r| !r.loaded));

        let again = E5Embeddings::new(config).expect("load E5 again");
        let reloaded = status.embedder().expect("reported");
        assert_eq!(
            reloaded.calibrated_at, report.calibrated_at,
            "not measured again"
        );
        assert_eq!(reloaded.pages_per_minute, report.pages_per_minute);
        let vector = again.embed_query("lease notice period").expect("embedded");
        assert_eq!(vector.len(), 768);
    }

    /// On a machine with a graphics card the provider starts on, its vectors
    /// match the CPU's: the same text, the same direction within a small
    /// tolerance. CI's runners have no such card, so this runs by hand
    /// (`cargo test -- --ignored`) on one that has.
    #[test]
    #[ignore = "needs a graphics card DirectML or Core ML starts on; CI has none"]
    fn a_graphics_card_embeds_like_the_cpu() {
        let cpu = E5Embeddings::new(variant_config(E5Variant::Fp16)).expect("load E5 on the CPU");
        let gpu = E5Embeddings::new(
            variant_config(E5Variant::Fp16).with_compute(ComputePreference::Auto),
        )
        .expect("load");
        assert!(gpu.accelerator().is_gpu(), "no graphics card ran the model");
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
