//! Installing the pinned search models (E5 embedder and reranker) into a
//! folder with the app's own verified downloader; already verified files are
//! kept. The answer-checking model is optional and installed only on request.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use shodh_rag::embeddings::model_store::{
    HttpByteSource, InstallPhase, InstallProgress, ModelStore,
};

async fn install_store(store: ModelStore) -> Result<()> {
    let source = HttpByteSource::new().context("creating the download client")?;
    // Print each artifact's phase changes and every 10% of a download.
    let last: Mutex<Option<(String, String, u64)>> = Mutex::new(None);
    let progress = move |p: &InstallProgress| {
        let phase = match p.phase {
            InstallPhase::Checking => "checking",
            InstallPhase::Downloading => "downloading",
            InstallPhase::Verifying => "verifying",
            InstallPhase::Verified => "verified",
        };
        let decile = (p.artifact_bytes * 10)
            .checked_div(p.artifact_total)
            .unwrap_or(0);
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        let current = (p.artifact.clone(), phase.to_string(), decile);
        if last.as_ref() != Some(&current) {
            eprintln!("{}: {phase} {}%", p.artifact, decile * 10);
            *last = Some(current);
        }
    };
    let report = store
        .install(&source, &progress)
        .await
        .context("installing the models")?;
    let downloaded = report.artifacts.iter().filter(|a| a.downloaded).count();
    eprintln!(
        "{} files verified ({downloaded} downloaded) in {}",
        report.artifacts.len(),
        store.root().display()
    );
    Ok(())
}

/// Install the search models into `dir`; with `answer_check`, also the
/// answer-checking (NLI) model used by the grounding verifier.
pub async fn install(dir: &Path, answer_check: bool) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    install_store(ModelStore::search_models(dir)).await?;
    if answer_check {
        install_store(ModelStore::answer_check_model(dir)).await?;
    }
    Ok(())
}
