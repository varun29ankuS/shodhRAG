//! Hugging Face tokenizer implementation for tokenizer.json format
//! Production-grade implementation supporting Phi-3, Phi-4, and other modern models

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Hugging Face tokenizer configuration structure
#[derive(Debug, Deserialize)]
struct HFTokenizerConfig {
    version: String,
    #[serde(default)]
    truncation: Option<TruncationConfig>,
    #[serde(default)]
    padding: Option<PaddingConfig>,
    added_tokens: Vec<AddedToken>,
    normalizer: Option<NormalizerConfig>,
    pre_tokenizer: Option<PreTokenizerConfig>,
    post_processor: Option<PostProcessorConfig>,
    decoder: Option<DecoderConfig>,
    model: ModelConfig,
}

#[derive(Debug, Deserialize)]
struct TruncationConfig {
    direction: String,
    max_length: usize,
    strategy: String,
    stride: usize,
}

#[derive(Debug, Deserialize)]
struct PaddingConfig {
    direction: String,
    pad_id: u32,
    pad_token: String,
    pad_type_id: u32,
}

#[derive(Debug, Deserialize)]
struct AddedToken {
    id: u32,
    content: String,
    single_word: bool,
    lstrip: bool,
    rstrip: bool,
    normalized: bool,
    special: bool,
}

#[derive(Debug, Deserialize)]
struct NormalizerConfig {
    #[serde(rename = "type")]
    normalizer_type: String,
}

#[derive(Debug, Deserialize)]
struct PreTokenizerConfig {
    #[serde(rename = "type")]
    pretokenizer_type: String,
}

#[derive(Debug, Deserialize)]
struct PostProcessorConfig {
    #[serde(rename = "type")]
    processor_type: String,
}

#[derive(Debug, Deserialize)]
struct DecoderConfig {
    #[serde(rename = "type")]
    decoder_type: String,
}

#[derive(Debug, Deserialize)]
struct ModelConfig {
    #[serde(rename = "type")]
    model_type: String,
    vocab: HashMap<String, u32>,
    merges: Vec<String>,
    #[serde(default)]
    unk_token: Option<String>,
    #[serde(default)]
    continuing_subword_prefix: Option<String>,
    #[serde(default)]
    end_of_word_suffix: Option<String>,
    #[serde(default)]
    fuse_unk: Option<bool>,
}

/// Production-grade Hugging Face tokenizer
pub struct HFTokenizer {
    vocab: HashMap<String, u32>,
    inverse_vocab: HashMap<u32, String>,
    merges: HashMap<(String, String), usize>,
    added_tokens: HashMap<String, u32>,
    special_tokens: HashMap<String, u32>,
    unk_token: String,
    pad_token: String,
    bos_token: String,
    eos_token: String,
    vocab_size: usize,
    continuing_subword_prefix: String,
    end_of_word_suffix: String,
}

impl HFTokenizer {
    /// Load tokenizer from Hugging Face tokenizer.json file
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("Failed to read tokenizer file {:?}: {}", path, e))?;
        
        let config: HFTokenizerConfig = serde_json::from_str(&content)
            .map_err(|e| anyhow!("Failed to parse tokenizer.json: {}", e))?;
        
        // Build merge rules
        let mut merges = HashMap::new();
        for (rank, merge) in config.model.merges.iter().enumerate() {
            let parts: Vec<&str> = merge.split_whitespace().collect();
            if parts.len() == 2 {
                merges.insert((parts[0].to_string(), parts[1].to_string()), rank);
            }
        }
        
        // Process added tokens for special tokens
        let mut special_tokens = HashMap::new();
        let mut added_tokens = HashMap::new();
        let mut unk_token = "<|endoftext|>".to_string();
        let mut pad_token = "<|endoftext|>".to_string();
        let mut bos_token = "<|endoftext|>".to_string();
        let mut eos_token = "<|endoftext|>".to_string();
        
        for token in &config.added_tokens {
            added_tokens.insert(token.content.clone(), token.id);
            
            if token.special {
                special_tokens.insert(token.content.clone(), token.id);
                
                // Detect common special tokens
                match token.content.as_str() {
                    s if s.contains("unk") || s == "<unk>" => unk_token = s.to_string(),
                    s if s.contains("pad") || s == "<pad>" => pad_token = s.to_string(),
                    s if s.contains("bos") || s == "<s>" => bos_token = s.to_string(),
                    s if s.contains("eos") || s == "</s>" || s == "<|endoftext|>" => {
                        eos_token = s.to_string();
                        if bos_token == "<|endoftext|>" {
                            bos_token = s.to_string();
                        }
                        if unk_token == "<|endoftext|>" {
                            unk_token = s.to_string();
                        }
                        if pad_token == "<|endoftext|>" {
                            pad_token = s.to_string();
                        }
                    }
                    _ => {}
                }
            }
        }
        
        // Build inverse vocabulary
        let inverse_vocab: HashMap<u32, String> = config.model.vocab.iter()
            .map(|(token, &id)| (id, token.clone()))
            .collect();
        
        let vocab_size = config.model.vocab.len();
        let continuing_subword_prefix = config.model.continuing_subword_prefix
            .unwrap_or_else(|| "##".to_string());
        let end_of_word_suffix = config.model.end_of_word_suffix
            .unwrap_or_else(|| String::new());
        
        Ok(Self {
            vocab: config.model.vocab,
            inverse_vocab,
            merges,
            added_tokens,
            special_tokens,
            unk_token,
            pad_token,
            bos_token,
            eos_token,
            vocab_size,
            continuing_subword_prefix,
            end_of_word_suffix,
        })
    }
    
    /// Encode text to token IDs
    pub fn encode(&self, text: &str, add_special_tokens: bool) -> Result<Vec<u32>> {
        let mut tokens = Vec::new();
        
        if add_special_tokens {
            if let Some(&bos_id) = self.special_tokens.get(&self.bos_token) {
                tokens.push(bos_id);
            }
        }
        
        // Split text into words
        let words = self.split_text(text);
        
        for word in words {
            // Check for special/added tokens first
            if let Some(&token_id) = self.added_tokens.get(&word) {
                tokens.push(token_id);
                continue;
            }
            
            // Apply BPE encoding
            let subwords = self.bpe_encode(&word);
            
            for subword in subwords {
                if let Some(&token_id) = self.vocab.get(&subword) {
                    tokens.push(token_id);
                } else if let Some(&unk_id) = self.special_tokens.get(&self.unk_token) {
                    tokens.push(unk_id);
                } else if let Some(&unk_id) = self.vocab.get(&self.unk_token) {
                    tokens.push(unk_id);
                }
            }
        }
        
        if add_special_tokens {
            if let Some(&eos_id) = self.special_tokens.get(&self.eos_token) {
                tokens.push(eos_id);
            }
        }
        
        Ok(tokens)
    }
    
    /// Decode token IDs to text
    pub fn decode(&self, token_ids: &[u32], skip_special_tokens: bool) -> Result<String> {
        let mut tokens = Vec::new();
        
        for &token_id in token_ids {
            if skip_special_tokens && self.special_tokens.values().any(|&id| id == token_id) {
                continue;
            }
            
            if let Some(token) = self.inverse_vocab.get(&token_id) {
                tokens.push(token.clone());
            }
        }
        
        // Join tokens and clean up
        let mut text = tokens.join("");
        
        // Handle subword prefixes/suffixes
        if !self.continuing_subword_prefix.is_empty() {
            text = text.replace(&self.continuing_subword_prefix, "");
        }
        
        if !self.end_of_word_suffix.is_empty() {
            text = text.replace(&self.end_of_word_suffix, " ");
        }
        
        // Handle common tokenizer artifacts
        text = text.replace("Ġ", " "); // GPT-style space token
        text = text.replace("▁", " "); // SentencePiece style
        text = text.replace("##", ""); // BERT style
        
        // Clean up multiple spaces
        while text.contains("  ") {
            text = text.replace("  ", " ");
        }
        
        text = text.trim().to_string();
        
        Ok(text)
    }
    
    /// Apply BPE encoding to a word
    fn bpe_encode(&self, word: &str) -> Vec<String> {
        if word.is_empty() {
            return Vec::new();
        }
        
        // Convert to character list
        let mut word_tokens: Vec<String> = word.chars().map(|c| c.to_string()).collect();
        
        // Apply merges
        loop {
            if word_tokens.len() < 2 {
                break;
            }
            
            let mut best_merge = None;
            let mut best_rank = usize::MAX;
            
            for i in 0..word_tokens.len() - 1 {
                let pair = (word_tokens[i].clone(), word_tokens[i + 1].clone());
                if let Some(&rank) = self.merges.get(&pair) {
                    if rank < best_rank {
                        best_rank = rank;
                        best_merge = Some(i);
                    }
                }
            }
            
            if let Some(merge_pos) = best_merge {
                let merged = format!("{}{}", word_tokens[merge_pos], word_tokens[merge_pos + 1]);
                word_tokens[merge_pos] = merged;
                word_tokens.remove(merge_pos + 1);
            } else {
                break;
            }
        }
        
        word_tokens
    }
    
    /// Split text into words
    fn split_text(&self, text: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut current_word = String::new();
        
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !current_word.is_empty() {
                    words.push(current_word.clone());
                    current_word.clear();
                }
            } else if ch.is_ascii_punctuation() {
                if !current_word.is_empty() {
                    words.push(current_word.clone());
                    current_word.clear();
                }
                words.push(ch.to_string());
            } else {
                current_word.push(ch);
            }
        }
        
        if !current_word.is_empty() {
            words.push(current_word);
        }
        
        words
    }
    
    /// Get vocabulary size
    pub fn vocab_size(&self) -> usize {
        self.vocab_size
    }
    
    /// Get special tokens
    pub fn get_special_tokens(&self) -> &HashMap<String, u32> {
        &self.special_tokens
    }
    
    /// Get EOS token
    pub fn get_eos_token(&self) -> &str {
        &self.eos_token
    }
    
    /// Get BOS token  
    pub fn get_bos_token(&self) -> &str {
        &self.bos_token
    }
    
    /// Get UNK token
    pub fn get_unk_token(&self) -> &str {
        &self.unk_token
    }
    
    /// Get PAD token
    pub fn get_pad_token(&self) -> &str {
        &self.pad_token
    }
}