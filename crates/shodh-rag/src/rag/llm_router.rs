//! LLM-Based Intent Router
//!
//! Replaces fragile rule-based intent detection with a single LLM call that
//! simultaneously handles: intent classification, query rewriting (coreference
//! resolution), and query expansion for multi-variant search.
//!
//! Falls back to rule-based logic when LLM is unavailable.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::query_rewriter::ConversationContext;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouterIntent {
    Search,
    CodeGeneration,
    General,
    AgentCreation,
    ToolAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterOutput {
    pub intent: RouterIntent,
    /// The rewritten query with pronouns resolved and context expanded.
    /// Identical to the original if no rewriting was needed.
    pub rewritten_query: String,
    /// 1-3 search query variants for multi-query retrieval.
    /// Empty when intent is not Search.
    pub search_queries: Vec<String>,
    /// Brief explanation of the routing decision (for logging/debugging).
    #[serde(default)]
    pub reasoning: String,
    /// Token usage and latency for the router call itself.
    #[serde(skip)]
    pub token_usage: RouterTokenUsage,
}

#[derive(Debug, Clone, Default)]
pub struct RouterTokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub latency_ms: u64,
}

// ---------------------------------------------------------------------------
// Router Prompt
// ---------------------------------------------------------------------------

const ROUTER_SYSTEM_PROMPT: &str = r#"You are a query router. Given a user message and conversation context, output a JSON object with exactly these fields:

{"intent":"search|code_generation|general|agent_creation|tool_action","rewritten_query":"...","search_queries":["..."],"reasoning":"..."}

RULES:
- "search": User wants information FROM their indexed documents (people, data, facts, comparisons, lookups).
- "code_generation": User explicitly asks to write/generate/implement code, functions, classes, APIs.
- "general": Greetings, meta questions ("who are you"), creative generation (diagrams, flowcharts, charts), calculations, general knowledge, or anything that does NOT need document retrieval or tool execution.
- "agent_creation": User wants to create, build, or design a specialized AI agent OR a crew/team of agents (e.g. "create an agent for legal review", "create a team to research and write about X", "build a crew for document analysis").
- "tool_action": User wants to perform an ACTION that requires executing a tool — creating/updating/deleting tasks, setting reminders, scheduling events, managing calendar items, reading/writing files. Any request that implies mutating data or performing a concrete action (not just retrieving information).
- rewritten_query: Resolve ALL pronouns (her/his/it/this/that/they) using conversation context. Make the query fully self-contained. If no rewriting needed, copy the original message verbatim.
- search_queries: For "search" intent ONLY, provide 1-3 diverse search queries (keyword extraction, synonyms, rephrased). Use empty array [] for all other intents.
- reasoning: One sentence explaining your decision.

CRITICAL:
- "create a task" / "add a reminder" / "schedule a meeting" / "remind me to" / "add to my todo" / "mark task as done" / "delete the event" = "tool_action", NOT "general" or "code_generation".
- "show me a flowchart" / "draw a diagram" / "visualize" = "general" (creative generation), NOT "search".
- "tell me more" / "what about her salary" / "and the PAN?" = resolve pronouns/ellipsis from context, then classify.
- Short messages like "hi", "thanks", "ok" = "general".
- "agent_creation" is ONLY for explicitly creating/building/designing a new agent definition. NOT for running/using an existing agent.
- "run it" / "run the agent" / "execute it" / "give me a summary" / "summarize this" = "search", NOT "agent_creation".
- If a previous message created an agent, follow-up questions are "search" (using the agent), not "agent_creation".

Output ONLY the JSON object, nothing else."#;

fn build_router_prompt(user_message: &str, context: &ConversationContext) -> String {
    let mut parts = Vec::with_capacity(5);
    parts.push(ROUTER_SYSTEM_PROMPT.to_string());

    // Inject conversation context (compact — last 4 turns)
    if !context.recent_messages.is_empty() {
        let history: String = context
            .recent_messages
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("\nConversation:\n{}", history));
    }

    if !context.entities.is_empty() {
        let entities: String = context
            .entities
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("Entities mentioned: {}", entities));
    }

    if !context.files_discussed.is_empty() {
        let files: String = context
            .files_discussed
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("Files discussed: {}", files));
    }

    parts.push(format!("\nUser message: \"{}\"\nJSON:", user_message));
    parts.join("\n")
}

// ---------------------------------------------------------------------------
// Response Parsing
// ---------------------------------------------------------------------------

/// Parse the LLM's JSON response into a RouterOutput.
/// Handles common LLM quirks: markdown fences, trailing text, partial JSON.
fn parse_router_response(raw: &str) -> Result<RouterOutput> {
    // Strip markdown code fences if present
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // Find the JSON object boundaries
    let json_str = match (cleaned.find('{'), cleaned.rfind('}')) {
        (Some(start), Some(end)) if end > start => &cleaned[start..=end],
        _ => cleaned,
    };

    // Strict parse first
    if let Ok(output) = serde_json::from_str::<RouterOutput>(json_str) {
        return Ok(output);
    }

    // Lenient parse: extract fields manually
    let intent = if json_str.contains("\"tool_action\"") {
        RouterIntent::ToolAction
    } else if json_str.contains("\"search\"") && !json_str.contains("\"agent_creation\"") {
        RouterIntent::Search
    } else if json_str.contains("\"code_generation\"") {
        RouterIntent::CodeGeneration
    } else if json_str.contains("\"agent_creation\"") {
        RouterIntent::AgentCreation
    } else {
        RouterIntent::General
    };

    let rewritten_query = extract_json_string(json_str, "rewritten_query").unwrap_or_default();

    let search_queries = extract_json_array(json_str, "search_queries").unwrap_or_default();

    let reasoning = extract_json_string(json_str, "reasoning")
        .unwrap_or_else(|| "LLM router (partial parse)".to_string());

    Ok(RouterOutput {
        intent,
        rewritten_query,
        search_queries,
        reasoning,
        token_usage: RouterTokenUsage::default(),
    })
}

/// Extract a JSON string field value by scanning for `"field":"value"`.
fn extract_json_string(json: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\"", field);
    let pos = json.find(&pattern)?;
    let after_key = &json[pos + pattern.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let trimmed = after_colon.trim_start();

    if !trimmed.starts_with('"') {
        return None;
    }

    // Find the closing quote, handling escaped quotes
    let content = &trimmed[1..];
    let mut end = 0;
    let mut escaped = false;
    for (i, ch) in content.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            end = i;
            break;
        }
    }

    if end > 0 {
        Some(content[..end].to_string())
    } else {
        None
    }
}

/// Extract a JSON string array field by scanning for `"field":["v1","v2"]`.
fn extract_json_array(json: &str, field: &str) -> Option<Vec<String>> {
    let pattern = format!("\"{}\"", field);
    let pos = json.find(&pattern)?;
    let after_key = &json[pos + pattern.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();

    if !after_colon.starts_with('[') {
        return None;
    }

    let bracket_end = after_colon.find(']')?;
    let arr_str = &after_colon[1..bracket_end];

    let items: Vec<String> = arr_str
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim().trim_matches('"');
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect();

    Some(items)
}

// ---------------------------------------------------------------------------
// Main Router Function
// ---------------------------------------------------------------------------

/// Route a user message using the LLM.
///
/// Returns `Ok(RouterOutput)` on success. Caller should fall back to rule-based
/// routing if this returns `Err`.
///
/// `llm_manager` must be a reference to an initialized LLMManager (behind the
/// async read lock). The function calls `generate_custom(prompt, 150)` for a
/// fast, short-form classification response.
pub async fn route_with_llm(
    user_message: &str,
    context: &ConversationContext,
    llm_manager: &crate::llm::LLMManager,
) -> Result<RouterOutput> {
    let prompt = build_router_prompt(user_message, context);
    let prompt_tokens = estimate_tokens(&prompt);

    let start = std::time::Instant::now();
    let raw_response = llm_manager
        .generate_custom(&prompt, 200)
        .await
        .context("LLM router call failed")?;
    let latency_ms = start.elapsed().as_millis() as u64;

    let completion_tokens = estimate_tokens(&raw_response);

    let mut output = parse_router_response(&raw_response)?;

    // Fill token usage
    output.token_usage = RouterTokenUsage {
        prompt_tokens,
        completion_tokens,
        latency_ms,
    };

    // Ensure search_queries is populated for Search intent
    if output.intent == RouterIntent::Search && output.search_queries.is_empty() {
        output.search_queries.push(output.rewritten_query.clone());
    }

    // Ensure rewritten_query is not empty
    if output.rewritten_query.trim().is_empty() {
        output.rewritten_query = user_message.to_string();
    }

    tracing::info!(
        intent = ?output.intent,
        rewritten = %output.rewritten_query,
        search_queries = ?output.search_queries,
        reasoning = %output.reasoning,
        prompt_tokens = prompt_tokens,
        completion_tokens = completion_tokens,
        latency_ms = latency_ms,
        "LLM router decision"
    );

    Ok(output)
}

/// Quick token estimate (chars / 4).
fn estimate_tokens(text: &str) -> usize {
    (text.len() + 3) / 4
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_json() {
        let raw = r#"{"intent":"search","rewritten_query":"anushree salary","search_queries":["anushree salary","anushree income"],"reasoning":"Looking up salary data"}"#;
        let output = parse_router_response(raw).unwrap();
        assert_eq!(output.intent, RouterIntent::Search);
        assert_eq!(output.rewritten_query, "anushree salary");
        assert_eq!(output.search_queries.len(), 2);
    }

    #[test]
    fn test_parse_json_with_fences() {
        let raw = "```json\n{\"intent\":\"general\",\"rewritten_query\":\"show flowchart\",\"search_queries\":[],\"reasoning\":\"Creative\"}\n```";
        let output = parse_router_response(raw).unwrap();
        assert_eq!(output.intent, RouterIntent::General);
    }

    #[test]
    fn test_parse_json_with_trailing_text() {
        let raw = r#"Here is the result: {"intent":"code_generation","rewritten_query":"write sorting function","search_queries":[],"reasoning":"Code request"} Hope that helps!"#;
        let output = parse_router_response(raw).unwrap();
        assert_eq!(output.intent, RouterIntent::CodeGeneration);
    }

    #[test]
    fn test_parse_partial_json() {
        let raw = r#"{"intent":"search","rewritten_query":"PAN number lookup"}"#;
        let output = parse_router_response(raw).unwrap();
        assert_eq!(output.intent, RouterIntent::Search);
        assert_eq!(output.rewritten_query, "PAN number lookup");
    }

    #[test]
    fn test_parse_garbage_defaults_to_general() {
        let raw = "I don't understand the format you want";
        let output = parse_router_response(raw).unwrap();
        assert_eq!(output.intent, RouterIntent::General);
    }

    #[test]
    fn test_parse_agent_creation() {
        let raw = r#"{"intent":"agent_creation","rewritten_query":"create a legal review agent","search_queries":[],"reasoning":"Agent creation request"}"#;
        let output = parse_router_response(raw).unwrap();
        assert_eq!(output.intent, RouterIntent::AgentCreation);
    }

    #[test]
    fn test_build_prompt_with_context() {
        let mut ctx = ConversationContext::default();
        ctx.recent_messages
            .push("user: who is anushree".to_string());
        ctx.entities.push("Anushree Sharma".to_string());

        let prompt = build_router_prompt("what is her salary", &ctx);
        assert!(prompt.contains("who is anushree"));
        assert!(prompt.contains("Anushree Sharma"));
        assert!(prompt.contains("what is her salary"));
    }

    #[test]
    fn test_build_prompt_empty_context() {
        let ctx = ConversationContext::default();
        let prompt = build_router_prompt("hello", &ctx);
        assert!(prompt.contains("hello"));
        assert!(!prompt.contains("Conversation:"));
    }

    #[test]
    fn test_parse_tool_action() {
        let raw = r#"{"intent":"tool_action","rewritten_query":"create a task to send email to paras tomorrow","search_queries":[],"reasoning":"User wants to create a task"}"#;
        let output = parse_router_response(raw).unwrap();
        assert_eq!(output.intent, RouterIntent::ToolAction);
    }

    #[test]
    fn test_parse_tool_action_lenient() {
        let raw = r#"{"intent":"tool_action","rewritten_query":"remind me to call john"}"#;
        let output = parse_router_response(raw).unwrap();
        assert_eq!(output.intent, RouterIntent::ToolAction);
    }
}
