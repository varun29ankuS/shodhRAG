//! MCP servers as agent tools: the `mcp.json` configuration ([`config`]), a
//! client for stdio and streamable HTTP servers ([`client`]) and the bridge
//! that turns each enabled server tool into a host tool ([`tool`]).

pub mod client;
pub mod config;
pub mod tool;

pub use client::{CallResult, McpClient, McpError, ToolInfo};
pub use config::{Approval, McpConfig, Mode, Scope, ServerConfig, Transport};
pub use tool::{
    effective_read_only, host_tool_name, server_tools, verified_for, McpCaller, McpTool, Verified,
};
