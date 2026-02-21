//! MCP Client implementation

use super::*;

// Re-export the main client from transport
pub use super::transport::StdioMCPClient;

// This module can be extended with additional client implementations
// (HTTP client, WebSocket client, etc.)
