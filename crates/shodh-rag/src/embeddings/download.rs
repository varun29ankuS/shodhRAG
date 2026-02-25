//! Auto-download embedding and reranker models from HuggingFace
//!
//! Downloads optimized ONNX variants on first run:
//! - intfloat/multilingual-e5-base (model_O4.onnx, ~555 MB)
//! - cross-encoder/ms-marco-MiniLM-L-6-v2 (model_O4.onnx, ~43 MB)

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

const HF_BASE: &str = "https://huggingface.co";

/// Model file descriptor: (relative_url_path, local_filename, expected_min_bytes)
struct ModelFile {
    url_path: &'static str,
    local_name: &'static str,
    min_bytes: u64,
}

/// E5 multilingual-e5-base files (Apache 2.0 license)
const E5_REPO: &str = "intfloat/multilingual-e5-base";
const E5_DIR: &str = "multilingual-e5-base";

const E5_FILES: &[ModelFile] = &[
    ModelFile {
        url_path: "onnx/model_O4.onnx",
        local_name: "model_O4.onnx",
        min_bytes: 100_000_000, // ~555 MB
    },
    ModelFile {
        url_path: "onnx/tokenizer.json",
        local_name: "tokenizer.json",
        min_bytes: 10_000, // ~17 MB
    },
];

/// Cross-encoder reranker files (Apache 2.0 license)
const RERANKER_REPO: &str = "cross-encoder/ms-marco-MiniLM-L-6-v2";
const RERANKER_DIR: &str = "ms-marco-MiniLM-L6-v2";

const RERANKER_FILES: &[ModelFile] = &[
    ModelFile {
        url_path: "onnx/model_O4.onnx",
        local_name: "model_O4.onnx",
        min_bytes: 10_000_000, // ~43 MB
    },
    ModelFile {
        // Tokenizer is at repo root for this model, not in onnx/
        url_path: "tokenizer.json",
        local_name: "tokenizer.json",
        min_bytes: 1_000, // ~700 KB
    },
];

/// Ensure the E5 embedding model is present, downloading if missing.
/// Returns the model directory path.
pub async fn ensure_e5_model(model_dir: &Path) -> Result<PathBuf> {
    let target_dir = model_dir.join(E5_DIR);
    ensure_model_files(&target_dir, E5_REPO, E5_FILES, "E5 multilingual-e5-base").await?;
    Ok(target_dir)
}

/// Ensure the cross-encoder reranker model is present, downloading if missing.
/// Returns the model directory path.
pub async fn ensure_reranker_model(model_dir: &Path) -> Result<PathBuf> {
    let target_dir = model_dir.join(RERANKER_DIR);
    ensure_model_files(
        &target_dir,
        RERANKER_REPO,
        RERANKER_FILES,
        "cross-encoder reranker",
    )
    .await?;
    Ok(target_dir)
}

/// Check if all required files exist; download any that are missing.
async fn ensure_model_files(
    target_dir: &Path,
    repo: &str,
    files: &[ModelFile],
    display_name: &str,
) -> Result<()> {
    // Check which files are missing or too small (corrupt download)
    let missing: Vec<&ModelFile> = files
        .iter()
        .filter(|f| {
            let path = target_dir.join(f.local_name);
            match path.metadata() {
                Ok(meta) => meta.len() < f.min_bytes,
                Err(_) => true,
            }
        })
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    tracing::info!(
        model = display_name,
        missing_files = missing.len(),
        dir = %target_dir.display(),
        "Auto-downloading model files from HuggingFace"
    );

    tokio::fs::create_dir_all(target_dir).await.map_err(|e| {
        anyhow!(
            "Failed to create model directory {}: {}",
            target_dir.display(),
            e
        )
    })?;

    let client = reqwest::Client::builder()
        .user_agent("shodh-rag/1.0")
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

    for file in &missing {
        let url = format!("{}/{}/resolve/main/{}", HF_BASE, repo, file.url_path);
        let dest = target_dir.join(file.local_name);
        download_with_retry(&client, &url, &dest, file.local_name, display_name).await?;
    }

    tracing::info!(
        model = display_name,
        "All model files downloaded successfully"
    );
    Ok(())
}

/// Download a file with retry and streaming progress.
async fn download_with_retry(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    filename: &str,
    model_name: &str,
) -> Result<()> {
    let max_retries = 3u32;
    let mut last_error = None;

    for attempt in 1..=max_retries {
        match download_streaming(client, url, dest, filename, model_name).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_error = Some(e);
                if attempt < max_retries {
                    let backoff = std::time::Duration::from_secs(2u64.pow(attempt));
                    tracing::warn!(
                        file = filename,
                        attempt,
                        "Download failed, retrying in {:?}",
                        backoff
                    );
                    tokio::time::sleep(backoff).await;
                    // Remove partial file
                    let _ = tokio::fs::remove_file(dest).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("Download failed after {} retries", max_retries)))
}

/// Stream download with periodic progress logging.
async fn download_streaming(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    filename: &str,
    model_name: &str,
) -> Result<()> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("HTTP request failed for {}: {}", filename, e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!(
            "HTTP {} downloading {} from {}",
            status,
            filename,
            url
        ));
    }

    let total_size = response.content_length().unwrap_or(0);
    let total_mb = total_size as f64 / 1_048_576.0;

    tracing::info!(
        model = model_name,
        file = filename,
        size_mb = format!("{:.1}", total_mb),
        "Downloading"
    );

    // Write to a temp file first, then rename (atomic-ish)
    let tmp_dest = dest.with_extension("downloading");
    let mut file = tokio::fs::File::create(&tmp_dest)
        .await
        .map_err(|e| anyhow!("Failed to create {}: {}", tmp_dest.display(), e))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_log_pct: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("Stream error downloading {}: {}", filename, e))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| anyhow!("Write error for {}: {}", filename, e))?;
        downloaded += chunk.len() as u64;

        // Log progress every 10%
        if total_size > 0 {
            let pct = (downloaded * 100) / total_size;
            if pct >= last_log_pct + 10 {
                last_log_pct = pct - (pct % 10);
                tracing::info!(
                    model = model_name,
                    file = filename,
                    progress = format!("{}%", last_log_pct),
                    downloaded_mb = format!("{:.1}", downloaded as f64 / 1_048_576.0),
                    "Download progress"
                );
            }
        }
    }

    file.flush().await?;
    drop(file);

    // Rename temp file to final destination
    tokio::fs::rename(&tmp_dest, dest)
        .await
        .map_err(|e| anyhow!("Failed to finalize {}: {}", filename, e))?;

    tracing::info!(
        model = model_name,
        file = filename,
        size_mb = format!("{:.1}", downloaded as f64 / 1_048_576.0),
        "Download complete"
    );

    Ok(())
}
