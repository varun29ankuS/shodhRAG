//! Model configuration with user-defined paths

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Model configuration with custom paths
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPathConfig {
    /// Base directory for models (can be overridden per model)
    pub base_path: Option<PathBuf>,

    /// Custom paths for specific models
    pub model_paths: HashMap<String, ModelPath>,

    /// Whether to use embedded models if available
    pub use_embedded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPath {
    /// Path to the ONNX model file
    pub model_file: PathBuf,

    /// Optional path to additional data files
    pub data_files: Vec<PathBuf>,

    /// Optional path to tokenizer
    pub tokenizer_path: Option<PathBuf>,
}

impl Default for ModelPathConfig {
    fn default() -> Self {
        Self {
            base_path: None,
            model_paths: HashMap::new(),
            use_embedded: false,
        }
    }
}

impl ModelPathConfig {
    /// Load configuration from a JSON file
    pub fn from_file(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Save configuration to a JSON file
    pub fn save_to_file(&self, path: &PathBuf) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Add a model path configuration
    pub fn add_model(&mut self, name: String, model_file: PathBuf, data_files: Vec<PathBuf>) {
        self.model_paths.insert(
            name,
            ModelPath {
                model_file,
                data_files,
                tokenizer_path: None,
            },
        );
    }

    /// Get model path for a specific model
    pub fn get_model_path(&self, model_name: &str) -> Option<&ModelPath> {
        self.model_paths.get(model_name)
    }

    /// Create example configuration file
    pub fn create_example() -> Self {
        let mut config = Self::default();

        // Add example Phi-3 configuration
        config.add_model(
            "phi3".to_string(),
            PathBuf::from("models/phi3-mini-4k-instruct-cpu-int4-rtn-block-32-acc-level-4.onnx"),
            vec![PathBuf::from(
                "models/phi3-mini-4k-instruct-cpu-int4-rtn-block-32-acc-level-4.onnx.data",
            )],
        );

        // Add example Phi-4 configuration
        config.add_model(
            "phi4".to_string(),
            PathBuf::from("models/phi4-mini/model.onnx"),
            vec![],
        );

        config
    }
}

/// Global model path configuration
static MODEL_CONFIG: std::sync::OnceLock<std::sync::Arc<parking_lot::RwLock<ModelPathConfig>>> =
    std::sync::OnceLock::new();

/// Initialize model configuration
pub fn init_model_config(config: ModelPathConfig) {
    MODEL_CONFIG.get_or_init(|| std::sync::Arc::new(parking_lot::RwLock::new(config)));
}

/// Get model configuration
pub fn get_model_config() -> std::sync::Arc<parking_lot::RwLock<ModelPathConfig>> {
    MODEL_CONFIG
        .get_or_init(|| {
            // Try to load from config file
            let config_path = PathBuf::from("model_paths.json");
            let config = if config_path.exists() {
                ModelPathConfig::from_file(&config_path).unwrap_or_default()
            } else {
                // Create default config and save example
                let example = ModelPathConfig::create_example();
                let _ = example.save_to_file(&PathBuf::from("model_paths.example.json"));
                ModelPathConfig::default()
            };
            std::sync::Arc::new(parking_lot::RwLock::new(config))
        })
        .clone()
}

/// Update model path at runtime
pub fn update_model_path(
    model_name: String,
    model_file: PathBuf,
    data_files: Vec<PathBuf>,
) -> Result<()> {
    let config = get_model_config();
    let mut config_write = config.write();
    config_write.add_model(model_name, model_file, data_files);

    // Save to file
    config_write.save_to_file(&PathBuf::from("model_paths.json"))?;
    Ok(())
}
