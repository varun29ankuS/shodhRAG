//! GroupQueryAttention KV Cache Management
//! Handles efficient caching for GQA-based models like Phi-3

use anyhow::{Result, anyhow};
use ndarray::{Array4, ArrayView4, Axis, IxDyn, ArrayViewD};
use std::collections::HashMap;

// Re-export for compatibility
pub type GQACacheConfig = GQAConfig;

/// Configuration for a GQA model
#[derive(Debug, Clone)]
pub struct GQAConfig {
    pub num_layers: usize,
    pub num_query_heads: usize,
    pub num_kv_heads: usize,  // num_kv_heads < num_query_heads for GQA
    pub head_dim: usize,
    pub max_seq_len: usize,
}

impl GQAConfig {
    /// Create config for Phi-3 Mini
    pub fn phi3_mini() -> Self {
        Self {
            num_layers: 32,
            num_query_heads: 32,
            num_kv_heads: 32,  // Phi-3 actually uses 32 KV heads (no grouping)
            head_dim: 96,
            max_seq_len: 32000,  // Extended context window for long documents
        }
    }
    
    /// Create config for models with actual GQA (like Llama 2)
    pub fn llama2_7b() -> Self {
        Self {
            num_layers: 32,
            num_query_heads: 32,
            num_kv_heads: 8,  // 4:1 grouping ratio
            head_dim: 128,
            max_seq_len: 4096,
        }
    }
}

/// GQA cache for efficient KV storage
pub struct GQACache {
    pub config: GQAConfig,
    
    // Key and value caches: [batch, num_kv_heads, seq_len, head_dim]
    pub key_cache: Vec<Array4<f32>>,
    pub value_cache: Vec<Array4<f32>>,
    
    // Current sequence length
    pub sequence_length: usize,
    pub current_seq_len: usize,
}

impl GQACache {
    /// Create new cache
    pub fn new(config: GQAConfig) -> Self {
        // Initialize empty caches for each layer
        let mut key_cache = Vec::with_capacity(config.num_layers);
        let mut value_cache = Vec::with_capacity(config.num_layers);
        
        for _ in 0..config.num_layers {
            // Start with minimal size, will grow as needed
            key_cache.push(Array4::zeros((1, config.num_kv_heads, 1, config.head_dim)));
            value_cache.push(Array4::zeros((1, config.num_kv_heads, 1, config.head_dim)));
        }
        
        Self {
            config,
            key_cache,
            value_cache,
            sequence_length: 0,
            current_seq_len: 0,
        }
    }
    
    /// Clear cache for new sequence
    pub fn clear(&mut self) {
        self.sequence_length = 0;
        self.current_seq_len = 0;
        
        // Reset all cache arrays
        for layer_idx in 0..self.config.num_layers {
            self.key_cache[layer_idx] = Array4::zeros((1, self.config.num_kv_heads, 1, self.config.head_dim));
            self.value_cache[layer_idx] = Array4::zeros((1, self.config.num_kv_heads, 1, self.config.head_dim));
        }
    }
    
    /// Get cache tensors as ndarray views for v2.0 API
    pub fn as_ndarray_inputs(&self) -> Vec<(&'static str, ArrayViewD<f32>)> {
        let mut inputs = Vec::with_capacity(self.config.num_layers * 2);
        
        for layer_idx in 0..self.config.num_layers {
            // Create name strings that will live as long as the returned vec
            let key_name = format!("past_key_{}", layer_idx);
            let value_name = format!("past_value_{}", layer_idx);
            
            // Box::leak creates a &'static str from String
            let key_name_static: &'static str = Box::leak(key_name.into_boxed_str());
            let value_name_static: &'static str = Box::leak(value_name.into_boxed_str());
            
            // Add key and value cache views
            inputs.push((key_name_static, self.key_cache[layer_idx].view().into_dyn()));
            inputs.push((value_name_static, self.value_cache[layer_idx].view().into_dyn()));
        }
        
        inputs
    }
    
    /// Update cache with new KV pairs from model output
    pub fn update_from_outputs(&mut self, outputs: &ort::session::SessionOutputs, new_seq_len: usize) -> Result<()> {
        // Update sequence length
        self.sequence_length = new_seq_len;
        self.current_seq_len = new_seq_len;
        
        // Process each layer's KV cache
        for layer_idx in 0..self.config.num_layers {
            // Try to get key and value outputs for this layer
            let key_name = format!("present.{}.key", layer_idx);
            let value_name = format!("present.{}.value", layer_idx);
            
            // Extract key tensor if available
            if let Some(key_output) = outputs.get(&key_name) {
                if let Ok((key_shape, key_data)) = key_output.try_extract_tensor::<f32>() {
                    // Validate shape: [batch, num_kv_heads, seq_len, head_dim]
                    if key_shape.len() == 4 && 
                       key_shape[1] == self.config.num_kv_heads as i64 && 
                       key_shape[3] == self.config.head_dim as i64 {
                        
                        // Update key cache
                        self.key_cache[layer_idx] = Array4::from_shape_vec(
                            (1, self.config.num_kv_heads, new_seq_len, self.config.head_dim),
                            key_data.to_vec()
                        )?;
                    }
                }
            }
            
            // Extract value tensor if available
            if let Some(value_output) = outputs.get(&value_name) {
                if let Ok((value_shape, value_data)) = value_output.try_extract_tensor::<f32>() {
                    // Validate shape: [batch, num_kv_heads, seq_len, head_dim]
                    if value_shape.len() == 4 && 
                       value_shape[1] == self.config.num_kv_heads as i64 && 
                       value_shape[3] == self.config.head_dim as i64 {
                        
                        // Update value cache
                        self.value_cache[layer_idx] = Array4::from_shape_vec(
                            (1, self.config.num_kv_heads, new_seq_len, self.config.head_dim),
                            value_data.to_vec()
                        )?;
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Get the number of cached tokens
    pub fn cached_tokens(&self) -> usize {
        self.sequence_length
    }
    
    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.sequence_length == 0
    }
    
    /// Get memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        let elements_per_layer = self.config.num_kv_heads * self.sequence_length * self.config.head_dim;
        let bytes_per_layer = elements_per_layer * std::mem::size_of::<f32>();
        
        // Key + Value for each layer
        bytes_per_layer * 2 * self.config.num_layers
    }
}