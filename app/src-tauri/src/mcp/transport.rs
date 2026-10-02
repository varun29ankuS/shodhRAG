//! MCP Transport implementations - Stdio, HTTP, WebSocket

use super::*;
use anyhow::{Context as AnyhowContext, Result};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

/// Stdio-based MCP client (most common)
pub struct StdioMCPClient {
    config: MCPServerConfig,
    process: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    stdout: Arc<Mutex<Option<BufReader<ChildStdout>>>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    request_id: Arc<std::sync::atomic::AtomicU64>,
}

impl StdioMCPClient {
    pub async fn new(config: MCPServerConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);

        // Set environment variables
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        // Configure stdio
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Spawn process
        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", config.name))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get stdout"))?;

        let client = Self {
            config,
            process: Arc::new(Mutex::new(Some(child))),
            stdin: Arc::new(Mutex::new(Some(stdin))),
            stdout: Arc::new(Mutex::new(Some(BufReader::new(stdout)))),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            request_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        };

        // Send initialization request
        client.initialize().await?;

        Ok(client)
    }

    async fn initialize(&self) -> Result<()> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_request_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {}
                },
                "clientInfo": {
                    "name": "shodh-rag",
                    "version": "0.1.0"
                }
            }
        });

        self.send_request(request).await?;
        let _response = self.receive_response().await?;

        // Send initialized notification
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });

        self.send_request(notification).await?;

        Ok(())
    }

    async fn send_request(&self, request: Value) -> Result<()> {
        let mut stdin_guard = self.stdin.lock().await;
        let stdin = stdin_guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Stdin not available"))?;

        let request_str = serde_json::to_string(&request)?;
        stdin.write_all(request_str.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        Ok(())
    }

    async fn receive_response(&self) -> Result<Value> {
        let mut stdout_guard = self.stdout.lock().await;
        let stdout = stdout_guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Stdout not available"))?;

        let mut line = String::new();
        stdout.read_line(&mut line).await?;

        if line.is_empty() {
            anyhow::bail!("Connection closed by server");
        }

        let response: Value = serde_json::from_str(&line)?;
        Ok(response)
    }

    fn next_request_id(&self) -> u64 {
        self.request_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl MCPClient for StdioMCPClient {
    async fn discover_tools(&mut self) -> Result<Vec<ToolDefinition>> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_request_id(),
            "method": "tools/list"
        });

        self.send_request(request).await?;
        let response = self.receive_response().await?;

        if let Some(result) = response.get("result") {
            if let Some(tools) = result.get("tools") {
                let tools: Vec<ToolDefinition> = serde_json::from_value(tools.clone())?;
                return Ok(tools);
            }
        }

        Ok(Vec::new())
    }

    async fn call_tool(&self, name: &str, params: Value) -> Result<ToolCallResult> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_request_id(),
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": params
            }
        });

        self.send_request(request).await?;
        let response = self.receive_response().await?;

        if let Some(error) = response.get("error") {
            return Ok(ToolCallResult {
                success: false,
                result: None,
                error: Some(error.to_string()),
                artifacts: Vec::new(),
            });
        }

        if let Some(result) = response.get("result") {
            return Ok(ToolCallResult {
                success: true,
                result: Some(result.clone()),
                error: None,
                artifacts: Vec::new(),
            });
        }

        anyhow::bail!("Invalid response from server");
    }

    async fn list_resources(&self) -> Result<Vec<Resource>> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_request_id(),
            "method": "resources/list"
        });

        self.send_request(request).await?;
        let response = self.receive_response().await?;

        if let Some(result) = response.get("result") {
            if let Some(resources) = result.get("resources") {
                let resources: Vec<Resource> = serde_json::from_value(resources.clone())?;
                return Ok(resources);
            }
        }

        Ok(Vec::new())
    }

    async fn read_resource(&self, uri: &str) -> Result<ResourceContent> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_request_id(),
            "method": "resources/read",
            "params": {
                "uri": uri
            }
        });

        self.send_request(request).await?;
        let response = self.receive_response().await?;

        if let Some(result) = response.get("result") {
            let content: ResourceContent = serde_json::from_value(result.clone())?;
            return Ok(content);
        }

        anyhow::bail!("Failed to read resource: {}", uri);
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn server_info(&self) -> &MCPServerConfig {
        &self.config
    }
}

impl Drop for StdioMCPClient {
    fn drop(&mut self) {
        // Mark as disconnected
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);

        // Kill process
        if let Ok(mut process_guard) = self.process.try_lock() {
            if let Some(mut process) = process_guard.take() {
                let _ = process.start_kill();
            }
        }
    }
}
