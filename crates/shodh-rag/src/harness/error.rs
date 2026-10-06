//! Typed harness errors. Every message says what failed and, where there is
//! one, what the user can do about it.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("The agent runtime (omp {version}) is not installed at {path}. Install it with the agent runtime download, or set SHODH_OMP_PATH to a verified copy.")]
    BinaryMissing {
        path: PathBuf,
        version: &'static str,
    },

    #[error("The agent runtime at {path} failed its integrity check (expected sha256 {expected}, found {actual}). Delete it and download it again.")]
    HashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("No pinned checksum exists for the agent runtime on this platform ({0}). Download it through Shodh so its checksum is recorded.")]
    NoPinnedHash(String),

    #[error("The agent runtime is not available for this platform ({0}).")]
    UnsupportedPlatform(String),

    #[error("Downloading the agent runtime failed: {0}")]
    Download(String),

    #[error("The local in-process model cannot drive the agent yet. Choose a cloud provider or Ollama in Settings → Models.")]
    UnsupportedLocalModel,

    #[error("No language model is configured. Choose a provider in Settings → Models.")]
    LlmDisabled,

    #[error("The {0} provider is not supported by the agent runtime. Choose OpenRouter, Anthropic, OpenAI, Google, xAI or Ollama.")]
    UnsupportedProvider(String),

    #[error("No API key is set for {0}. Add it in Settings → Models.")]
    MissingApiKey(&'static str),

    #[error("The model id {0:?} is not valid.")]
    InvalidModel(String),

    #[error("The model {0} is not allowed: stealth models may log prompts for training. Choose another model, or select it again in the model picker and confirm that you accept this risk.")]
    DisallowedModel(String),

    #[error("Local-only mode is on, so the agent cannot use {0}, which sends data off this computer. Choose a local model (Ollama) in Settings → Models, or turn off Local-only mode in Settings → Privacy.")]
    LocalOnlyCloudModel(String),

    #[error("Unknown agent profile {0:?}.")]
    UnknownProfile(String),

    #[error("Starting the agent runtime failed: {0}")]
    Spawn(String),

    #[error("The agent runtime did not become ready: {0}")]
    NotReady(String),

    #[error("The agent runtime rejected {command}: {error}")]
    CommandFailed { command: String, error: String },

    #[error("The agent runtime did not answer {0} in time.")]
    Timeout(String),

    #[error("The agent session has ended. Start a new conversation turn to restart it.")]
    SessionClosed,

    #[error("An answer is already running. Steer it or interrupt it first.")]
    RunInProgress,

    #[error("Messages starting with '/' are reserved for runtime commands. Rephrase the message.")]
    SlashCommand,

    #[error("The message is empty.")]
    EmptyMessage,

    #[error(
        "The message is too long ({0} characters). Shorten it or attach the text as a document."
    )]
    MessageTooLong(usize),

    #[error("No approval is pending for step {0}.")]
    NoPendingApproval(String),

    #[error("Agent session {0} does not exist.")]
    UnknownSession(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Could not encode a message for the agent runtime: {0}")]
    Encode(#[from] serde_json::Error),
}
