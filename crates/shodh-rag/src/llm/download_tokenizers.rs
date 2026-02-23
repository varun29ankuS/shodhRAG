//! Download tokenizer files for LLM models

use anyhow::{Result, anyhow};
use std::path::Path;
use tokio::fs;
use reqwest;

/// Tokenizer URLs for different models - Production Grade with Commercial Licenses
const TOKENIZER_URLS: &[(&str, &str)] = &[
    // Microsoft Phi-3 (MIT License - Commercial Use OK)
    ("phi3_tokenizer.json", "https://huggingface.co/microsoft/Phi-3-mini-4k-instruct/resolve/main/tokenizer.json"),
    ("phi3_tokenizer_config.json", "https://huggingface.co/microsoft/Phi-3-mini-4k-instruct/resolve/main/tokenizer_config.json"),
    ("phi3_special_tokens_map.json", "https://huggingface.co/microsoft/Phi-3-mini-4k-instruct/resolve/main/special_tokens_map.json"),

    // Mistral 7B v0.3 (Apache 2.0 License - Commercial Use OK)
    ("mistral_tokenizer.json", "https://huggingface.co/mistralai/Mistral-7B-v0.3/resolve/main/tokenizer.json"),
    ("mistral_tokenizer_config.json", "https://huggingface.co/mistralai/Mistral-7B-v0.3/resolve/main/tokenizer_config.json"),

    // Microsoft Orca-2 (MIT License - Commercial Use OK)
    // Note: Orca-2 uses Llama tokenizer format
    ("orca2_tokenizer.model", "https://huggingface.co/microsoft/Orca-2-7b/resolve/main/tokenizer.model"),
    ("orca2_tokenizer_config.json", "https://huggingface.co/microsoft/Orca-2-7b/resolve/main/tokenizer_config.json"),

    // Alibaba Qwen2.5 (Apache 2.0 License - Commercial Use OK)
    ("qwen_merges.txt", "https://huggingface.co/Qwen/Qwen2.5-0.5B/resolve/main/merges.txt"),
    ("qwen_vocab.json", "https://huggingface.co/Qwen/Qwen2.5-0.5B/resolve/main/vocab.json"),
    ("qwen_tokenizer_config.json", "https://huggingface.co/Qwen/Qwen2.5-0.5B/resolve/main/tokenizer_config.json"),

    // Google Gemma (Gemma License - Commercial Use with Registration)
    ("gemma_tokenizer.model", "https://huggingface.co/google/gemma-2b/resolve/main/tokenizer.model"),
    ("gemma_tokenizer_config.json", "https://huggingface.co/google/gemma-2b/resolve/main/tokenizer_config.json"),
];

/// Download all tokenizers
pub async fn download_all_tokenizers(cache_dir: &Path) -> Result<()> {
    tracing::info!("Downloading tokenizers to {:?}", cache_dir);

    // Create cache directory if it doesn't exist
    fs::create_dir_all(cache_dir).await?;

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let mut success_count = 0;
    let mut failed_downloads = Vec::new();

    for (filename, url) in TOKENIZER_URLS {
        let file_path = cache_dir.join(filename);

        if file_path.exists() {
            tracing::debug!("Tokenizer {} already exists", filename);
            success_count += 1;
            continue;
        }

        tracing::info!("Downloading {} ...", filename);

        // Try up to 3 times with exponential backoff
        let mut retry_count = 0;
        let max_retries = 3;
        let mut last_error = None;

        while retry_count < max_retries {
            match download_file(&client, url, &file_path).await {
                Ok(_) => {
                    tracing::info!("Successfully downloaded {}", filename);
                    success_count += 1;
                    break;
                },
                Err(e) => {
                    retry_count += 1;
                    last_error = Some(e.to_string());

                    if retry_count < max_retries {
                        let wait_time = std::time::Duration::from_secs(2u64.pow(retry_count));
                        tracing::warn!("Retry {}/{} after {:?}...", retry_count, max_retries, wait_time);
                        tokio::time::sleep(wait_time).await;
                    }
                }
            }
        }

        if retry_count >= max_retries {
            tracing::error!("Failed to download {}: {}", filename, last_error.unwrap_or_default());
            failed_downloads.push(filename.to_string());
        }
    }

    tracing::info!(
        "Tokenizer download complete: {}/{} successful",
        success_count, TOKENIZER_URLS.len()
    );

    if !failed_downloads.is_empty() {
        tracing::warn!(
            "Failed to download {} tokenizers: {:?}. The system will use fallback tokenizers.",
            failed_downloads.len(), failed_downloads
        );
    }

    Ok(())
}

/// Download a single file
async fn download_file(client: &reqwest::Client, url: &str, path: &Path) -> Result<()> {
    let response = client.get(url).send().await?;

    if !response.status().is_success() {
        return Err(anyhow!("Failed to download: HTTP {}", response.status()));
    }

    let content = response.bytes().await?;
    fs::write(path, content).await?;

    Ok(())
}

/// Download tokenizer for a specific model
pub async fn download_model_tokenizer(model: &str, cache_dir: &Path) -> Result<()> {
    let (json_file, config_file) = match model {
        "phi3" => ("phi3_tokenizer.json", "phi3_tokenizer_config.json"),
        "mistral" => ("mistral_tokenizer.json", "mistral_tokenizer_config.json"),
        "orca2" => ("orca2_tokenizer.json", "orca2_tokenizer_config.json"),
        _ => return Err(anyhow!("Unknown model: {}", model)),
    };

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    // Find URLs
    let json_url = TOKENIZER_URLS.iter()
        .find(|(name, _)| *name == json_file)
        .map(|(_, url)| *url)
        .ok_or_else(|| anyhow!("URL not found for {}", json_file))?;

    let config_url = TOKENIZER_URLS.iter()
        .find(|(name, _)| *name == config_file)
        .map(|(_, url)| *url)
        .ok_or_else(|| anyhow!("URL not found for {}", config_file))?;

    // Download files
    fs::create_dir_all(cache_dir).await?;

    let json_path = cache_dir.join(json_file);
    let config_path = cache_dir.join(config_file);

    if !json_path.exists() {
        tracing::info!("Downloading {}", json_file);
        download_file(&client, json_url, &json_path).await?;
    }

    if !config_path.exists() {
        tracing::info!("Downloading {}", config_file);
        download_file(&client, config_url, &config_path).await?;
    }

    Ok(())
}
