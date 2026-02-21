//! ReAct Tool-Calling Loop
//!
//! Sends messages + tool schemas to the LLM, executes any requested tool calls,
//! feeds results back, and loops until the LLM produces a final text response.
//! Works with any provider that supports `chat()` (OpenAI, Anthropic, Google, Ollama).

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::llm::{
    ChatMessage, ChatResponse, ChatStreamEvent, LLMManager, ToolCall, ToolSchema,
};
use super::tools::{AgentTool, ToolRegistry};
use super::context::AgentContext;

/// Configuration for the tool-calling loop.
#[derive(Debug, Clone)]
pub struct ToolLoopConfig {
    /// Maximum number of LLM round-trips (tool call → result → re-send).
    pub max_iterations: usize,
    /// Per-tool execution timeout in seconds.
    pub tool_timeout_secs: u64,
    /// If true, emit streaming events via the callback.
    pub streaming: bool,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            tool_timeout_secs: 30,
            streaming: true,
        }
    }
}

/// A single tool invocation record for observability.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolInvocation {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: String,
    pub success: bool,
    pub duration_ms: u64,
}

/// The final output of a tool-calling loop run.
#[derive(Debug, Clone)]
pub struct ToolLoopResult {
    /// The LLM's final text response.
    pub content: String,
    /// All tool invocations that occurred during the loop.
    pub tool_invocations: Vec<ToolInvocation>,
    /// Total number of LLM round-trips.
    pub iterations: usize,
}

/// Callback for streaming events during the loop.
pub trait ToolLoopEmitter: Send + Sync {
    fn on_content_delta(&self, delta: &str);
    fn on_tool_start(&self, tool_name: &str, arguments: &str);
    fn on_tool_complete(&self, invocation: &ToolInvocation);
    fn on_thinking(&self, message: &str);
}

/// Run the ReAct tool-calling loop.
///
/// 1. Send `messages` + `tool_schemas` to the LLM via `chat()`.
/// 2. If the LLM returns `ToolCalls` → execute each tool → append results → loop.
/// 3. If the LLM returns `Content` → done.
pub async fn run_tool_loop(
    llm: &LLMManager,
    tool_registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    tool_schemas: &[ToolSchema],
    agent_context: &AgentContext,
    config: &ToolLoopConfig,
    emitter: Option<&dyn ToolLoopEmitter>,
) -> Result<ToolLoopResult> {
    let mut invocations = Vec::new();
    let mut iterations = 0;

    loop {
        iterations += 1;
        if iterations > config.max_iterations {
            tracing::warn!(
                max = config.max_iterations,
                "Tool loop hit max iterations, forcing text response"
            );
            // Ask LLM to respond without tools
            let response = llm.chat(messages, &[], ).await?;
            let content = match response {
                ChatResponse::Content(text) => text,
                ChatResponse::ToolCalls(_) => {
                    "I was unable to complete the task within the allowed number of tool calls. \
                     Here is what I found so far based on the tool results above."
                        .to_string()
                }
            };
            return Ok(ToolLoopResult {
                content,
                tool_invocations: invocations,
                iterations,
            });
        }

        tracing::debug!(iteration = iterations, "Tool loop: sending to LLM");

        if let Some(em) = emitter {
            if iterations > 1 {
                em.on_thinking("Analyzing tool results...");
            }
        }

        // Call LLM with tools
        let response = llm.chat(messages, tool_schemas).await?;

        match response {
            ChatResponse::Content(text) => {
                tracing::debug!(iteration = iterations, "Tool loop: LLM returned content, done");
                return Ok(ToolLoopResult {
                    content: text,
                    tool_invocations: invocations,
                    iterations,
                });
            }
            ChatResponse::ToolCalls(tool_calls) => {
                tracing::info!(
                    iteration = iterations,
                    count = tool_calls.len(),
                    tools = ?tool_calls.iter().map(|tc| &tc.name).collect::<Vec<_>>(),
                    "Tool loop: LLM requested tool calls"
                );

                // Append the assistant's tool call message to history
                messages.push(ChatMessage::assistant_tool_calls(tool_calls.clone()));

                // Execute each tool call
                for tc in &tool_calls {
                    if let Some(em) = emitter {
                        em.on_tool_start(&tc.name, &tc.arguments);
                    }

                    let start = std::time::Instant::now();
                    let result = execute_tool_call(
                        tool_registry,
                        tc,
                        agent_context,
                        config.tool_timeout_secs,
                    )
                    .await;
                    let duration_ms = start.elapsed().as_millis() as u64;

                    let (output, success) = match result {
                        Ok(tool_result) => (tool_result.output.clone(), tool_result.success),
                        Err(e) => (format!("Tool execution error: {}", e), false),
                    };

                    let invocation = ToolInvocation {
                        tool_name: tc.name.clone(),
                        arguments: serde_json::from_str(&tc.arguments)
                            .unwrap_or(serde_json::json!({})),
                        result: output.clone(),
                        success,
                        duration_ms,
                    };

                    if let Some(em) = emitter {
                        em.on_tool_complete(&invocation);
                    }

                    invocations.push(invocation);

                    // Append tool result message
                    messages.push(ChatMessage::tool_result(
                        &tc.id,
                        &tc.name,
                        &output,
                    ));
                }
            }
        }
    }
}

/// Streaming variant: yields content deltas and tool events via the channel.
/// Returns the accumulated ToolLoopResult when done.
pub async fn run_tool_loop_stream(
    llm: &LLMManager,
    tool_registry: &ToolRegistry,
    messages: &mut Vec<ChatMessage>,
    tool_schemas: &[ToolSchema],
    agent_context: &AgentContext,
    config: &ToolLoopConfig,
    event_tx: tokio::sync::mpsc::Sender<ToolLoopEvent>,
) -> Result<ToolLoopResult> {
    let mut invocations = Vec::new();
    let mut iterations = 0;

    loop {
        iterations += 1;
        if iterations > config.max_iterations {
            let response = llm.chat(messages, &[]).await?;
            let content = match response {
                ChatResponse::Content(text) => text,
                ChatResponse::ToolCalls(_) => "Max tool iterations reached.".to_string(),
            };
            let _ = event_tx.send(ToolLoopEvent::Done).await;
            return Ok(ToolLoopResult {
                content,
                tool_invocations: invocations,
                iterations,
            });
        }

        // Use streaming chat
        let mut rx = llm.chat_stream(messages, tool_schemas).await?;

        let mut content_acc = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        while let Some(event) = rx.recv().await {
            match event {
                ChatStreamEvent::ContentDelta(delta) => {
                    content_acc.push_str(&delta);
                    let _ = event_tx
                        .send(ToolLoopEvent::ContentDelta(delta))
                        .await;
                }
                ChatStreamEvent::ToolCallComplete(tc) => {
                    let _ = event_tx
                        .send(ToolLoopEvent::ToolCallRequested {
                            name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        })
                        .await;
                    tool_calls.push(tc);
                }
                ChatStreamEvent::Done => break,
            }
        }

        // If LLM returned content (no tool calls), we're done
        if tool_calls.is_empty() {
            let _ = event_tx.send(ToolLoopEvent::Done).await;
            return Ok(ToolLoopResult {
                content: content_acc,
                tool_invocations: invocations,
                iterations,
            });
        }

        // LLM wants tool calls — execute them
        messages.push(ChatMessage::assistant_tool_calls(tool_calls.clone()));

        for tc in &tool_calls {
            let start = std::time::Instant::now();
            let result = execute_tool_call(
                tool_registry,
                tc,
                agent_context,
                config.tool_timeout_secs,
            )
            .await;
            let duration_ms = start.elapsed().as_millis() as u64;

            let (output, success) = match result {
                Ok(tr) => (tr.output.clone(), tr.success),
                Err(e) => (format!("Tool error: {}", e), false),
            };

            let invocation = ToolInvocation {
                tool_name: tc.name.clone(),
                arguments: serde_json::from_str(&tc.arguments)
                    .unwrap_or(serde_json::json!({})),
                result: output.clone(),
                success,
                duration_ms,
            };

            let _ = event_tx
                .send(ToolLoopEvent::ToolCallCompleted(invocation.clone()))
                .await;

            invocations.push(invocation);
            messages.push(ChatMessage::tool_result(&tc.id, &tc.name, &output));
        }
    }
}

/// Events emitted during a streaming tool loop.
#[derive(Debug, Clone)]
pub enum ToolLoopEvent {
    /// A token of the LLM's text response.
    ContentDelta(String),
    /// The LLM requested a tool call (before execution).
    ToolCallRequested { name: String, arguments: String },
    /// A tool call completed.
    ToolCallCompleted(ToolInvocation),
    /// The loop is finished.
    Done,
}

/// Execute a single tool call against the registry.
async fn execute_tool_call(
    registry: &ToolRegistry,
    tool_call: &ToolCall,
    agent_context: &AgentContext,
    timeout_secs: u64,
) -> Result<super::tools::ToolResult> {
    let tool = registry
        .get(&tool_call.name)
        .ok_or_else(|| anyhow!("Unknown tool: {}", tool_call.name))?;

    let parameters: serde_json::Value =
        serde_json::from_str(&tool_call.arguments).unwrap_or(serde_json::json!({}));

    let input = super::tools::ToolInput {
        tool_id: tool_call.name.clone(),
        parameters,
    };

    let future = tool.execute(input, agent_context.clone());

    match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        future,
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Ok(super::tools::ToolResult {
            success: false,
            output: format!("Tool '{}' timed out after {}s", tool_call.name, timeout_secs),
            data: serde_json::json!({}),
            error: Some("timeout".to_string()),
        }),
    }
}

/// Convert ToolDescriptions from the registry into ToolSchemas for the LLM.
pub fn tool_descriptions_to_schemas(descriptions: &[super::tools::ToolDescription]) -> Vec<ToolSchema> {
    descriptions
        .iter()
        .map(|d| ToolSchema {
            name: d.id.clone(),
            description: d.description.clone(),
            parameters: d.parameters_schema.clone(),
        })
        .collect()
}
