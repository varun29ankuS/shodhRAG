//! Time a llama.cpp chat generation alone and while Core ML embeds full
//! batches, and how fast Core ML embeds meanwhile.
//!
//! ```text
//! cargo run --release -p shodh-rag --example chat_contention -- <model.gguf> <models_dir> [runs]
//! ```
//!
//! `models_dir` holds `multilingual-e5-base/` with `model_O4.onnx` and
//! `tokenizer.json`. Core ML writes its graph and compiled model there, so
//! use a copy, not the app's folder. Apple silicon only.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use shodh_rag::compute::provider::ComputePreference;
use shodh_rag::embeddings::e5::{E5Config, E5Embeddings, E5Mode};
use shodh_rag::llm::{GenerationConfig, LLMProvider, LlamaCppProvider};

const PROMPT: &str = "Write a long, detailed essay on how stone arch bridges were built \
    in the Roman empire, the tools and materials used, and why many still stand today.";

const PASSAGE: &str = "The committee reviewed the annual maintenance report for the building \
    and its grounds. The roof was inspected in the spring and two sections of flashing were \
    replaced after the winter storms. The heating system was serviced in October; its boiler is \
    now eleven years old and the engineer recommends budgeting for a replacement within three \
    years. Water pressure on the upper floors dropped twice during the summer, which the city \
    traced to work on the main line. Residents asked for more bicycle parking, better lighting \
    in the courtyard and a clearer schedule for the laundry room.";

async fn decode_speed(llm: &LlamaCppProvider, max_tokens: usize) -> Result<(usize, f64)> {
    let config = GenerationConfig {
        max_tokens,
        temperature: 0.7,
        top_p: 0.95,
        top_k: 40,
        repetition_penalty: 1.1,
        stop_sequences: Vec::new(),
        seed: Some(7),
        reasoning: None,
    };
    let mut stream = llm.generate_stream(PROMPT, &config).await?;
    let mut first = None;
    let mut tokens = 0usize;
    while let Some(token) = stream.next().await {
        token.map_err(|e| anyhow::anyhow!("{e:?}"))?;
        first.get_or_insert_with(Instant::now);
        tokens += 1;
    }
    let elapsed = first.map_or(Duration::ZERO, |f| f.elapsed());
    Ok((
        tokens,
        (tokens.saturating_sub(1)) as f64 / elapsed.as_secs_f64(),
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        bail!("usage: chat_contention <model.gguf> <models_dir> [runs]");
    }
    let runs: usize = args.get(2).map_or(Ok(3), |r| r.parse())?;
    let llm = LlamaCppProvider::new(&PathBuf::from(&args[0]))?;
    decode_speed(&llm, 16).await?;

    for run in 0..runs {
        let (tokens, speed) = decode_speed(&llm, 256).await?;
        println!("chat alone, run {run}: {tokens} tokens at {speed:.1} tok/s");
    }

    let config = E5Config::auto_detect(&PathBuf::from(&args[1]))
        .context("no E5 model in that folder")?
        .with_compute(ComputePreference::Auto)
        .with_core_ml(true);
    let embedder = Arc::new(E5Embeddings::new(config)?);
    let started = Instant::now();
    embedder.wait_until_prepared();
    println!(
        "Core ML prepared in {:.0} s, holds the model: {}",
        started.elapsed().as_secs_f64(),
        embedder.holds_core_ml()
    );
    if !embedder.holds_core_ml() {
        bail!("Core ML did not keep the model");
    }

    let stop = Arc::new(AtomicBool::new(false));
    let batches = Arc::new(AtomicUsize::new(0));
    let indexing = {
        let (embedder, stop, batches) = (embedder.clone(), stop.clone(), batches.clone());
        std::thread::spawn(move || -> Result<()> {
            let texts: Vec<String> = (0..8)
                .map(|i| format!("Item {i}. {PASSAGE} {PASSAGE}"))
                .collect();
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            while !stop.load(Ordering::Relaxed) {
                embedder.embed_batch_with_mode(&texts, E5Mode::Passage)?;
                batches.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        })
    };

    let idle = Instant::now();
    let before = batches.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(10)).await;
    println!(
        "embedding alone: {:.1} batches/s",
        (batches.load(Ordering::Relaxed) - before) as f64 / idle.elapsed().as_secs_f64()
    );

    for run in 0..runs {
        let at = Instant::now();
        let before = batches.load(Ordering::Relaxed);
        let (tokens, speed) = decode_speed(&llm, 256).await?;
        println!(
            "chat while indexing, run {run}: {tokens} tokens at {speed:.1} tok/s; \
             embedding {:.1} batches/s meanwhile",
            (batches.load(Ordering::Relaxed) - before) as f64 / at.elapsed().as_secs_f64()
        );
    }

    stop.store(true, Ordering::Relaxed);
    indexing.join().expect("indexing thread")?;
    Ok(())
}
