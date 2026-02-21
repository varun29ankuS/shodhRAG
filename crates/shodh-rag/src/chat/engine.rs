use anyhow::Result;
use chrono::{Datelike, Timelike, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock as AsyncRwLock;
use uuid::Uuid;

use crate::agent::{
    AgentContext, AgentDefinition, AgentSystem, ConversationTurn, PersonalAssistant,
    ToolDescription, ToolInput, ToolRegistry, ToolResult, UserInfo,
};
use crate::llm::LLMManager;
use crate::memory::{
    CodeContext, ContextId, ConversationContext as MemConversationContext, DocumentContext,
    EnvironmentContext, Experience, ExperienceType, Memory, MemorySystem, ProjectContext, Query,
    RetrievalMode, RichContext, SemanticContext, TemporalContext, UserContext,
};
use crate::rag::{
    compress_history, format_compressed_history, ConversationContext as RagConversationContext,
    QueryAnalyzer, QueryIntent as RagQueryIntent, QueryRewriter,
};
use crate::rag_engine::RAGEngine;

use super::{
    build_corpus_stats, estimate_tokens, extract_artifacts, force_bullet_format,
    validate_citations, AssistantResponse, ChatContext, Citation,
    ConversationMessage, EventEmitter, Intent, ResponseMetadata, SearchResult, UserMessage,
    CODE_GENERATION_PROMPT, GENERAL_CHAT_PROMPT, RAG_SYSTEM_PROMPT,
};
use crate::rag::structured_output::STRUCTURED_OUTPUT_INSTRUCTIONS;

pub struct ChatEngine {
    rag: Arc<AsyncRwLock<RAGEngine>>,
    agent_system: Arc<AsyncRwLock<Option<Arc<AsyncRwLock<AgentSystem>>>>>,
    personal_assistant: Arc<AsyncRwLock<PersonalAssistant>>,
    llm_manager: Option<Arc<AsyncRwLock<Option<LLMManager>>>>,
    memory: Arc<AsyncRwLock<MemorySystem>>,
    tool_registry: Arc<ToolRegistry>,
}

impl ChatEngine {
    pub async fn new(
        rag: Arc<AsyncRwLock<RAGEngine>>,
        agent_system: Arc<AsyncRwLock<Option<Arc<AsyncRwLock<AgentSystem>>>>>,
        personal_assistant: Arc<AsyncRwLock<PersonalAssistant>>,
        llm_manager: Option<Arc<AsyncRwLock<Option<LLMManager>>>>,
        memory: Arc<AsyncRwLock<MemorySystem>>,
    ) -> Self {
        let tool_registry = Arc::new(ToolRegistry::new());
        // Inject live RAG engine so tools (e.g. rag_search) can search documents
        tool_registry.set_rag_engine(rag.clone()).await;
        // Also inject into calendar store so task/event mutations trigger semantic indexing
        tool_registry.set_calendar_rag_engine(rag.clone()).await;
        Self {
            rag,
            agent_system,
            personal_assistant,
            llm_manager,
            memory,
            tool_registry,
        }
    }

    /// Set the calendar store's file path so calendar tools persist data.
    pub async fn set_calendar_path(&self, path: std::path::PathBuf) {
        self.tool_registry.set_calendar_path(path).await;
    }

    /// Get the tool registry (for external access to calendar store, etc.)
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }

    pub fn get_available_tools(&self) -> Vec<ToolDescription> {
        self.tool_registry.get_tool_descriptions()
    }

    pub async fn execute_tool(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
        context: &ChatContext,
    ) -> Result<ToolResult> {
        let tool = self
            .tool_registry
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", tool_name))?;

        let agent_context = AgentContext {
            query: None,
            conversation_history: Vec::new(),
            variables: HashMap::new(),
            user_info: Some(UserInfo::new("default_user".to_string())),
            space_id: context.space_id.clone(),
            session_id: context
                .conversation_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            metadata: HashMap::new(),
        };

        let tool_input = ToolInput {
            tool_id: tool_name.to_string(),
            parameters,
        };

        tool.execute(tool_input, agent_context).await
    }

    /// Main entry point — process user message and return response.
    pub async fn process_message(
        &self,
        message: UserMessage,
        context: ChatContext,
        emitter: Option<&dyn EventEmitter>,
    ) -> Result<AssistantResponse> {
        let start_time = std::time::Instant::now();

        // 1. Retrieve relevant memories
        let relevant_memories = self.retrieve_relevant_memories(&message).await?;
        tracing::debug!("Retrieved {} relevant memories", relevant_memories.len());

        // 2. Detect intent (LLM router first, rule-based fallback)
        let (intent, router_output) = self.detect_intent(&message, &context).await?;

        // 3. Route to handler
        let mut response = match intent {
            Intent::Search => {
                self.handle_search(&message, &context, &relevant_memories, emitter, router_output)
                    .await?
            }
            Intent::CodeGeneration => self.handle_code_generation(&message, &context).await?,
            Intent::AgentChat => self.handle_agent_chat(&message, &context, emitter).await?,
            Intent::AgentCreation => {
                self.handle_agent_creation(&message, &context, emitter)
                    .await?
            }
            Intent::ToolAction => {
                self.handle_tool_action(&message, &context, emitter).await?
            }
            Intent::General => self.handle_general_chat(&message, &context).await?,
        };

        // 4. Store in memory
        self.store_conversation_memory(&message, &response, &context)
            .await?;

        // 5. Extract artifacts and strip their blocks from content
        let (artifacts, cleaned_content) = extract_artifacts(&response.content);
        if !artifacts.is_empty() {
            response.content = cleaned_content;
        }
        response.artifacts = artifacts;

        // 6. Metadata
        response.metadata.intent = intent;
        response.metadata.duration_ms = Some(start_time.elapsed().as_millis() as u64);

        Ok(response)
    }

    // ========================================================================
    // Intent Detection
    // ========================================================================

    async fn detect_intent(
        &self,
        message: &UserMessage,
        context: &ChatContext,
    ) -> Result<(Intent, Option<crate::rag::RouterOutput>)> {
        // Priority 1: Explicit agent selected (deterministic — user picked an agent)
        if context.agent_id.is_some() {
            return Ok((Intent::AgentChat, None));
        }

        // Priority 1.5: Deterministic tool-action detection — catches obvious
        // action requests (task creation, reminders, calendar ops) before the LLM
        // router has a chance to misclassify them as agent_creation or general.
        let content_lower = message.content.to_lowercase();
        if Self::is_tool_action(&content_lower) {
            tracing::info!(
                message = %message.content,
                "Deterministic tool-action detection matched"
            );
            return Ok((Intent::ToolAction, None));
        }

        // Priority 2: LLM Router — let the LLM classify intent, rewrite query,
        // and generate search variants in a single call.
        let conversation_ctx = Self::build_conversation_context(context);

        if let Some(llm_arc) = self.llm_manager.as_ref() {
            let llm_guard = llm_arc.read().await;
            if let Some(ref llm_manager) = *llm_guard {
                match crate::rag::llm_router::route_with_llm(
                    &message.content,
                    &conversation_ctx,
                    llm_manager,
                )
                .await
                {
                    Ok(router_output) => {
                        let intent = match router_output.intent {
                            crate::rag::RouterIntent::Search => Intent::Search,
                            crate::rag::RouterIntent::CodeGeneration => Intent::CodeGeneration,
                            crate::rag::RouterIntent::General => Intent::General,
                            crate::rag::RouterIntent::AgentCreation => Intent::AgentCreation,
                            crate::rag::RouterIntent::ToolAction => Intent::ToolAction,
                        };
                        return Ok((intent, Some(router_output)));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "LLM router failed, falling back to rule-based intent detection");
                    }
                }
            }
        }

        // ============================================================
        // FALLBACK: Rule-based intent detection (when LLM unavailable)
        // ============================================================
        let content_lower = message.content.to_lowercase();

        if Self::is_agent_creation(&content_lower) {
            return Ok((Intent::AgentCreation, None));
        }

        if Self::is_tool_action(&content_lower) {
            return Ok((Intent::ToolAction, None));
        }

        if Self::is_content_generation(&content_lower) {
            return Ok((Intent::General, None));
        }

        if Self::is_code_generation(&content_lower) {
            return Ok((Intent::CodeGeneration, None));
        }

        let rag = self.rag.read().await;
        let corpus_stats = build_corpus_stats(&rag, context.space_id.as_deref()).await?;
        drop(rag);

        let analyzer = QueryAnalyzer::new();
        let analysis = analyzer.analyze(&message.content, &corpus_stats);

        tracing::debug!(
            "Fallback QueryAnalyzer: intent={:?}, should_retrieve={}, confidence={:.2}",
            analysis.intent, analysis.decision.should_retrieve, analysis.decision.confidence
        );

        Ok((
            Self::map_rag_intent(analysis.intent, corpus_stats.total_docs),
            None,
        ))
    }

    fn is_agent_creation(query_lower: &str) -> bool {
        // Exclude "run" / "execute" / "use" requests — those are agent execution, not creation
        let execution_patterns = [
            "run it", "run the agent", "run agent", "execute", "use the agent",
            "use it", "use agent", "start the agent", "start agent",
        ];
        if execution_patterns.iter().any(|p| query_lower.contains(p)) {
            return false;
        }

        let creation_patterns = [
            "create an agent", "create a agent", "create agent",
            "make an agent", "make a agent", "make agent",
            "build an agent", "build a agent", "build agent",
            "i need an agent", "i need a agent",
            "generate an agent", "generate a agent", "generate agent",
            "new agent",
            "create a team", "create a crew", "build a team", "build a crew",
            "make a team", "make a crew", "assemble a team", "assemble a crew",
        ];
        let purpose_keywords = [
            "for analyzing", "for reviewing", "for summarizing", "for coding",
            "for research", "for legal", "for medical", "for financial",
            "that helps", "that specializes", "specialized in", "to help", "to handle",
        ];

        let has_creation = creation_patterns.iter().any(|p| query_lower.contains(p));
        let has_purpose = purpose_keywords.iter().any(|k| query_lower.contains(k));
        has_creation || (query_lower.contains("agent") && has_purpose)
    }

    /// Detect requests that require executing a tool (task creation, reminders,
    /// calendar events, file operations, etc.)
    fn is_tool_action(query_lower: &str) -> bool {
        let action_patterns = [
            "create a task", "create task", "add a task", "add task",
            "new task", "make a task", "add to my todo", "add to todo",
            "remind me", "set a reminder", "set reminder", "create a reminder",
            "schedule a meeting", "schedule meeting", "create an event", "create event",
            "add an event", "add event", "new event", "delete task", "remove task",
            "delete event", "remove event", "mark as done", "mark as complete",
            "complete the task", "update task", "update the task", "change the task",
            "add a subtask", "add subtask", "list my tasks", "show my tasks",
            "what are my tasks", "what's on my todo",
        ];
        action_patterns.iter().any(|p| query_lower.contains(p))
    }

    /// Detect requests to generate diagrams, flowcharts, charts, visualizations,
    /// or other creative content that the LLM should produce — NOT search for.
    fn is_content_generation(query_lower: &str) -> bool {
        let generation_verbs = [
            "show", "draw", "create", "make", "generate", "build",
            "design", "sketch", "illustrate", "render", "produce",
        ];
        let content_nouns = [
            "flowchart", "flow chart", "diagram", "visualization",
            "architecture diagram", "sequence diagram", "class diagram",
            "er diagram", "entity relationship", "mind map", "mindmap",
            "org chart", "organization chart", "pie chart", "bar chart",
            "gantt chart", "timeline", "infographic", "wireframe",
            "mermaid", "graph", "tree diagram", "state diagram",
        ];

        // "show me a flowchart" / "draw a diagram" / "create a visualization"
        let has_verb = generation_verbs.iter().any(|v| query_lower.contains(v));
        let has_noun = content_nouns.iter().any(|n| query_lower.contains(n));

        // Also catch: "flowchart for X" / "diagram of X" (noun-first, no verb)
        let starts_with_noun = content_nouns.iter().any(|n| query_lower.starts_with(n));

        (has_verb && has_noun) || starts_with_noun
    }

    fn is_code_generation(query_lower: &str) -> bool {
        let gen = ["generate", "create", "write", "implement", "build"];
        let code = [
            "code", "function", "class", "method", "api", "endpoint",
            "component", "module", "script", "program", "algorithm",
            "struct", "interface", "trait", "type", "enum",
        ];
        gen.iter().any(|k| query_lower.contains(k)) && code.iter().any(|k| query_lower.contains(k))
    }

    /// RAG-FIRST: Use documents unless it's clearly a non-document query.
    fn map_rag_intent(rag_intent: RagQueryIntent, _total_docs: usize) -> Intent {
        use RagQueryIntent::*;
        match rag_intent {
            Greeting | SimpleAcknowledgment => Intent::General,
            MetaQuestion | Clarification => Intent::General,
            CreativeGeneration | ExampleCreation => Intent::CodeGeneration,
            _ => Intent::Search,
        }
    }

    // ========================================================================
    // Handlers
    // ========================================================================

    async fn handle_search(
        &self,
        message: &UserMessage,
        context: &ChatContext,
        relevant_memories: &[Memory],
        emitter: Option<&dyn EventEmitter>,
        router_output: Option<crate::rag::RouterOutput>,
    ) -> Result<AssistantResponse> {
        let rag = self.rag.read().await;

        // Use router output if available (LLM-routed), else fall back to rule-based
        let (primary_query, expanded_queries, router_token_usage) =
            if let Some(ref ro) = router_output {
                let primary = ro.rewritten_query.clone();
                let queries = if ro.search_queries.is_empty() {
                    vec![primary.clone()]
                } else {
                    ro.search_queries.clone()
                };

                if primary != message.content {
                    tracing::info!(
                        original = %message.content,
                        rewritten = %primary,
                        reasoning = %ro.reasoning,
                        "Using LLM-routed query"
                    );
                }

                (primary, queries, Some(&ro.token_usage))
            } else {
                // Fallback: rule-based rewriting + expansion
                let conversation_ctx = Self::build_conversation_context(context);
                let rewriter = QueryRewriter::new();
                let rewritten = rewriter.rewrite_rule_based(&message.content, &conversation_ctx);
                let primary = rewritten.rewritten_query.clone();
                let expanded = rewriter.expand_query(&primary, &conversation_ctx);

                if rewritten.used_context {
                    tracing::info!(
                        original = %message.content,
                        rewritten = %primary,
                        "Fallback rule-based query rewrite"
                    );
                }

                (primary, expanded, None)
            };

        let max_results = context.max_results.unwrap_or(8);
        tracing::info!(
            primary_query = %primary_query,
            variant_count = expanded_queries.len(),
            max_results = max_results,
            "ChatEngine: starting multi-variant search"
        );

        // Search all variants and merge results
        let mut results = if expanded_queries.len() > 1 {
            let mut all_result_sets = Vec::new();
            for variant in &expanded_queries {
                match rag.search(variant, max_results).await {
                    Ok(variant_results) => {
                        tracing::debug!(
                            variant = %variant,
                            hits = variant_results.len(),
                            "Variant search complete"
                        );
                        all_result_sets.push(variant_results);
                    }
                    Err(e) => {
                        tracing::warn!(variant = %variant, error = %e, "Variant search failed");
                    }
                }
            }
            Self::merge_expanded_results(all_result_sets, max_results)
        } else {
            rag.search(&primary_query, max_results).await?
        };

        // Drop RAG read lock before acquiring LLM lock for reranking
        drop(rag);

        // LLM-based reranking: judge relevance to the original user question
        let mut rerank_latency_ms = None;
        if results.len() > 1 {
            if let Some(llm_arc) = self.llm_manager.as_ref() {
                let llm_guard = llm_arc.read().await;
                if let Some(ref llm_manager) = *llm_guard {
                    let rerank_start = std::time::Instant::now();
                    results = crate::reranking::llm_rerank(
                        llm_manager,
                        &message.content,
                        results,
                    ).await;
                    let elapsed = rerank_start.elapsed().as_millis() as u64;
                    rerank_latency_ms = Some(elapsed);
                    tracing::info!(
                        duration_ms = elapsed,
                        result_count = results.len(),
                        "LLM reranking of merged results complete"
                    );
                }
            }
        }

        tracing::info!(results_count = results.len(), "ChatEngine: search complete");

        // Convert to SearchResult
        let search_results: Vec<SearchResult> = results
            .iter()
            .map(|r| {
                let snippet_text: String = r.text.chars().take(200).collect();
                let source_file = r
                    .metadata
                    .get("file_path")
                    .or_else(|| r.metadata.get("source_file"))
                    .or_else(|| r.metadata.get("original_path"))
                    .cloned()
                    .unwrap_or_else(|| {
                        r.source
                            .strip_prefix("Folder: ")
                            .unwrap_or(&r.source)
                            .to_string()
                    });

                SearchResult {
                    text: r.text.clone(),
                    score: r.score,
                    source_file,
                    page_number: r.citation.as_ref().and_then(|c| c.page_numbers.clone()),
                    line_range: None,
                    snippet: snippet_text.clone(),
                    citation: r.citation.as_ref().map(|c| Citation {
                        title: c.title.clone(),
                        snippet: snippet_text.clone(),
                        score: r.score,
                        url: c.url.clone(),
                        authors: c.authors.clone(),
                        source: r.source.clone(),
                        year: c.year.clone(),
                        page_numbers: c.page_numbers.clone(),
                    }),
                }
            })
            .collect();

        let mut metadata = ResponseMetadata {
            model: None,
            input_tokens: None,
            output_tokens: None,
            duration_ms: None,
            intent: Intent::Search,
            router_tokens: router_token_usage.map(|t| t.prompt_tokens + t.completion_tokens),
            router_latency_ms: router_token_usage.map(|t| t.latency_ms),
            search_queries_used: Some(expanded_queries.clone()),
            rerank_latency_ms,
        };

        // Grounding: refuse when no results found
        if search_results.is_empty() {
            return Ok(AssistantResponse {
                content: "I could not find relevant information about this in your indexed documents. \
                          Try rephrasing your question, or ensure the relevant documents have been indexed."
                    .to_string(),
                artifacts: Vec::new(),
                citations: Vec::new(),
                suggestions: vec![
                    "Try a broader search term".to_string(),
                    "Check which folders are indexed".to_string(),
                    "Rephrase with specific keywords from the document".to_string(),
                ],
                search_results: Some(Vec::new()),
                metadata,
            });
        }

        let best_score = search_results.iter().map(|r| r.score).fold(0.0f32, f32::max);
        let low_confidence = best_score < 0.2;

        // === Context Curation Pipeline ===
        // Goal: send only chunks that add genuine information value.
        // Three stages: relevance filter → content dedup → information gain cutoff.

        let pre_filter_count = search_results.len();

        // Stage 1: Relevance filter — drop chunks scoring below 30% of best result.
        // A chunk at 0.25 when the best is 0.85 is noise, not signal.
        let score_threshold = best_score * 0.30;
        let mut search_results: Vec<SearchResult> = search_results
            .into_iter()
            .filter(|r| r.score >= score_threshold)
            .collect();

        // Stage 2: Content deduplication — chunks from overlapping document regions
        // or multi-variant search often contain near-identical text. Keep only the
        // highest-scored version when two chunks share >60% of their words.
        search_results = Self::deduplicate_by_content(search_results);

        // Stage 3: Score-gap cutoff — if there's a sharp relevance drop between
        // consecutive chunks (>40% relative drop), everything below that cliff
        // is unlikely to help the LLM and just wastes tokens.
        search_results = Self::cut_at_score_cliff(search_results);

        tracing::info!(
            best_score = best_score,
            score_threshold = score_threshold,
            pre_filter = pre_filter_count,
            post_curation = search_results.len(),
            "Context curation complete"
        );

        let num_sources = search_results.len();

        // Build context for LLM, annotating spreadsheet data for chart generation
        let context_text: String = search_results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let citation_info = r
                    .citation
                    .as_ref()
                    .map(|c| format!(" [Source: {}]", c.title))
                    .unwrap_or_default();

                // Hint for spreadsheet/table data so LLM knows it can generate charts
                let data_hint = if r.text.contains("| --- |") || r.text.contains("|---|") {
                    let has_numbers = r.text.lines().skip(2).any(|line| {
                        line.split('|')
                            .any(|cell| cell.trim().parse::<f64>().is_ok())
                    });
                    if has_numbers {
                        " [DATA: This is tabular data with numeric columns — suitable for chart visualization]"
                    } else {
                        ""
                    }
                } else {
                    ""
                };

                format!("[{}]{}{}\n{}", i + 1, citation_info, data_hint, r.text)
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        // Generate LLM response
        let mut content = if let Some(llm_guard_opt) = self.llm_manager.as_ref() {
            let llm_guard = llm_guard_opt.read().await;
            if let Some(llm_manager) = llm_guard.as_ref() {
                // Build history with rolling summarization
                let history_text = Self::build_history_text(context);

                // Build memory context
                let memory_text = Self::build_memory_text(relevant_memories);

                // Context window management
                let context_window = Self::get_context_window_from_llm(llm_manager);
                let system_prompt_budget = 2000;
                let response_budget = 4096;
                let query_budget = estimate_tokens(&message.content) + 100;
                let available = context_window
                    .saturating_sub(system_prompt_budget)
                    .saturating_sub(response_budget)
                    .saturating_sub(query_budget);

                let context_budget = (available * 60) / 100;
                let history_budget = (available * 25) / 100;
                let memory_budget = (available * 15) / 100;

                let context_text = Self::truncate_context_to_budget(&context_text, context_budget);
                let history_text = Self::truncate_to_budget(&history_text, history_budget);
                let memory_text = Self::truncate_to_budget(&memory_text, memory_budget);

                let prompt = format!(
                    "{instructions}\n\n\
                    ===== DOCUMENT CONTEXT (your ONLY source of facts) =====\n\
                    {context}\n\
                    ===== END OF DOCUMENT CONTEXT =====\n\n\
                    {history}{memory}\
                    User Question: \"{question}\"\n\n\
                    IMPORTANT REMINDER: Answer using ONLY facts from the DOCUMENT CONTEXT above. \
                    Do NOT use conversation history, memory, or your own knowledge as sources of facts. \
                    If information is not in the DOCUMENT CONTEXT, say you don't have it.\n\n\
                    Answer:",
                    instructions = context.custom_system_prompt.as_ref()
                        .map(|custom| format!("{}\n\n{}", custom, RAG_SYSTEM_PROMPT))
                        .unwrap_or_else(|| RAG_SYSTEM_PROMPT.to_string()),
                    context = context_text,
                    history = history_text,
                    memory = memory_text,
                    question = message.content,
                );

                let model_name = llm_manager
                    .info()
                    .map(|info| info.model)
                    .unwrap_or_else(|| "Unknown".to_string());

                let start_time = std::time::Instant::now();

                // Streaming mode if emitter provided.
                // Cap the LLM generation at 90 seconds — if the provider is
                // unreachable, fall back to showing search results directly
                // rather than hanging the UI.
                let generation_timeout = std::time::Duration::from_secs(90);
                let llm_response = if emitter.is_some() {
                    match tokio::time::timeout(
                        generation_timeout,
                        llm_manager.generate_stream(&prompt),
                    ).await {
                        Ok(Ok(mut token_stream)) => {
                            let mut accumulated = String::new();
                            while let Some(token) = token_stream.next().await {
                                accumulated.push_str(&token);
                                if let Some(em) = emitter {
                                    em.emit(
                                        "chat_token",
                                        serde_json::json!({
                                            "token": token,
                                            "accumulated": &accumulated,
                                        }),
                                    );
                                }
                            }
                            if let Some(em) = emitter {
                                em.emit(
                                    "chat_complete",
                                    serde_json::json!({ "content": &accumulated }),
                                );
                            }
                            Ok(accumulated)
                        }
                        Ok(Err(e)) => Err(e),
                        Err(_) => {
                            tracing::warn!("LLM generation timed out after 90s");
                            Err(anyhow::anyhow!("LLM generation timed out"))
                        }
                    }
                } else {
                    match tokio::time::timeout(
                        generation_timeout,
                        llm_manager.generate(&prompt),
                    ).await {
                        Ok(result) => result,
                        Err(_) => {
                            tracing::warn!("LLM generation timed out after 90s");
                            Err(anyhow::anyhow!("LLM generation timed out"))
                        }
                    }
                };

                match llm_response {
                    Ok(response_text) => {
                        let duration = start_time.elapsed();
                        metadata.input_tokens = Some(estimate_tokens(&prompt));
                        metadata.output_tokens = Some(estimate_tokens(&response_text));
                        metadata.duration_ms = Some(duration.as_millis() as u64);
                        metadata.model = Some(model_name);
                        response_text
                    }
                    Err(e) => {
                        tracing::warn!("LLM generation failed: {}, falling back to results", e);
                        Self::format_fallback_results(&search_results)
                    }
                }
            } else {
                Self::format_fallback_results(&search_results)
            }
        } else {
            Self::format_fallback_results(&search_results)
        };

        // Post-processing
        content = force_bullet_format(&content);
        content = validate_citations(&content, num_sources);

        if low_confidence {
            content = format!(
                "> **Note:** The retrieved documents have low relevance to your query. \
                The following answer may be incomplete.\n\n{}",
                content
            );
        }

        Ok(AssistantResponse {
            content,
            artifacts: Vec::new(),
            citations: search_results.iter().filter_map(|r| r.citation.clone()).collect(),
            suggestions: vec![
                "Tell me more".to_string(),
                "Show related information".to_string(),
            ],
            search_results: Some(search_results),
            metadata,
        })
    }

    async fn handle_code_generation(
        &self,
        message: &UserMessage,
        context: &ChatContext,
    ) -> Result<AssistantResponse> {
        let rag = self.rag.read().await;
        let similar_code = rag.search(&message.content, 5).await.ok();
        drop(rag);

        let code_context = similar_code
            .as_ref()
            .map(|results| {
                results
                    .iter()
                    .map(|r| r.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default();

        let llm_guard_opt = self
            .llm_manager
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not configured"))?;
        let llm_guard = llm_guard_opt.read().await;
        let llm_manager = llm_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not initialized"))?;

        let code_instructions = context.custom_system_prompt.as_ref()
            .map(|custom| format!("{}\n\n{}", custom, CODE_GENERATION_PROMPT))
            .unwrap_or_else(|| CODE_GENERATION_PROMPT.to_string());
        let prompt = format!(
            "{}\n\nContext from codebase:\n{}\n\nUser request: {}",
            code_instructions, code_context, message.content
        );

        let response = llm_manager
            .generate(&prompt)
            .await
            .map_err(|e| anyhow::anyhow!("LLM generation failed: {}", e))?;

        let model_name = llm_manager
            .info()
            .map(|info| info.model)
            .unwrap_or_else(|| "llm".to_string());

        Ok(AssistantResponse {
            content: response,
            artifacts: Vec::new(),
            citations: Vec::new(),
            suggestions: vec![
                "Add error handling".to_string(),
                "Generate tests".to_string(),
                "Explain code".to_string(),
            ],
            search_results: None,
            metadata: ResponseMetadata {
                model: Some(model_name),
                intent: Intent::CodeGeneration,
                ..Default::default()
            },
        })
    }

    async fn handle_agent_chat(
        &self,
        message: &UserMessage,
        context: &ChatContext,
        emitter: Option<&dyn EventEmitter>,
    ) -> Result<AssistantResponse> {
        let agent_id = context
            .agent_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No agent selected"))?;

        let agent_system_option_guard = self.agent_system.read().await;
        let agent_system_arc = agent_system_option_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Agent system not initialized"))?;

        let agent_system = agent_system_arc.read().await;

        let agent_context = AgentContext {
            query: Some(message.content.clone()),
            user_info: None,
            space_id: context.space_id.clone(),
            session_id: context
                .conversation_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            conversation_history: context
                .conversation_history
                .clone()
                .unwrap_or_default()
                .iter()
                .map(|msg| ConversationTurn {
                    role: msg.role.clone(),
                    content: msg.content.clone(),
                    timestamp: Utc::now().timestamp_millis() as u64,
                    metadata: HashMap::new(),
                })
                .collect(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };

        if let Some(em) = emitter {
            em.emit(
                "agent_execution_started",
                serde_json::json!({
                    "agent_id": agent_id,
                    "message": "Agent execution started...",
                }),
            );
        }

        let result = agent_system
            .execute_agent(agent_id, agent_context)
            .await
            .map_err(|e| anyhow::anyhow!("Agent execution failed: {}", e))?;

        if let Some(em) = emitter {
            em.emit(
                "agent_execution_complete",
                serde_json::json!({
                    "agent_id": agent_id,
                    "success": result.success,
                }),
            );
        }

        Ok(AssistantResponse {
            content: result.response,
            artifacts: Vec::new(),
            citations: Vec::new(),
            suggestions: Vec::new(),
            search_results: None,
            metadata: ResponseMetadata {
                model: Some(format!("agent:{}", agent_id)),
                intent: Intent::AgentChat,
                ..Default::default()
            },
        })
    }

    async fn handle_agent_creation(
        &self,
        message: &UserMessage,
        _context: &ChatContext,
        emitter: Option<&dyn EventEmitter>,
    ) -> Result<AssistantResponse> {
        let emit = |stage: &str, msg: &str, progress: u32| {
            if let Some(em) = emitter {
                em.emit(
                    "agent_creation_progress",
                    serde_json::json!({
                        "stage": stage,
                        "message": msg,
                        "progress": progress,
                    }),
                );
            }
        };

        // Check if user wants a crew/team rather than a single agent
        let content_lower = message.content.to_lowercase();
        let crew_keywords = ["crew", "team of agents", "multiple agents", "group of agents",
            "assemble a team", "build a team", "create a team", "make a team"];
        if crew_keywords.iter().any(|kw| content_lower.contains(kw)) {
            return self.handle_crew_creation(message, _context, emitter).await;
        }

        emit("starting", "Starting AI-powered agent creation...", 10);

        let llm_guard_opt = self
            .llm_manager
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not configured"))?;
        let llm_guard = llm_guard_opt.read().await;
        let llm_manager = llm_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not initialized"))?;

        emit("analyzing", "Analyzing request and generating agent definition...", 30);

        let agent_gen_prompt = format!(
            r#"You are an AI agent designer. Create a detailed agent definition based on the user's request.

User Request: "{}"

Generate a JSON agent definition with this EXACT structure:
{{
  "id": "unique-agent-id",
  "name": "AgentName",
  "description": "Brief description",
  "system_prompt": "Detailed system prompt (200-500 words)",
  "config": {{
    "temperature": 0.7,
    "max_tokens": 2000,
    "top_p": 0.9,
    "stream": true,
    "max_tool_calls": 5
  }},
  "capabilities": ["RAGSearch", "CodeAnalysis"],
  "tools": [
    {{
      "tool_id": "rag_search",
      "enabled": true,
      "config": {{}},
      "description": null
    }}
  ],
  "enabled": true,
  "metadata": {{}}
}}

Return ONLY the JSON object, no markdown code blocks."#,
            message.content
        );

        emit("generating", "Generating agent definition with AI...", 50);

        let llm_response = llm_manager
            .generate(&agent_gen_prompt)
            .await
            .map_err(|e| anyhow::anyhow!("Agent definition generation failed: {}", e))?;

        emit("parsing", "Parsing agent definition...", 60);

        let agent_def_json = llm_response
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let agent_def: AgentDefinition = serde_json::from_str(agent_def_json)
            .map_err(|e| anyhow::anyhow!("Failed to parse agent definition: {}", e))?;

        emit(
            "registering",
            &format!("Registering agent '{}'...", agent_def.name),
            75,
        );

        let agent_system_option_guard = self.agent_system.read().await;
        let agent_system_arc = agent_system_option_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Agent system not initialized"))?;
        let agent_system = agent_system_arc.write().await;
        agent_system
            .register_agent(agent_def.clone())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to register agent: {}", e))?;

        emit("saving", "Saving agent to disk...", 90);

        // Save to agents directory
        let agents_dir = std::env::current_dir()
            .ok()
            .and_then(|p| p.parent().map(|parent| parent.join("agents")))
            .unwrap_or_else(|| std::path::PathBuf::from("../agents"));

        if !agents_dir.exists() {
            std::fs::create_dir_all(&agents_dir)?;
        }

        let agent_file = agents_dir.join(format!("{}.yaml", agent_def.name.to_lowercase()));
        let yaml_content = serde_json::to_string_pretty(&agent_def)?;
        std::fs::write(&agent_file, &yaml_content)?;

        emit(
            "complete",
            &format!("Agent '{}' created successfully!", agent_def.name),
            100,
        );

        let tools_str = agent_def
            .tools
            .iter()
            .map(|t| t.tool_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let capabilities_str = agent_def
            .capabilities
            .iter()
            .map(|c| format!("{:?}", c))
            .collect::<Vec<_>>()
            .join(", ");

        // Auto-execute the agent if the user's request implies they want a result
        // e.g. "create an agent to summarize this space" -> create + run
        let task_keywords = ["summarize", "summary", "analyze", "review", "extract",
            "compare", "find", "search", "list", "describe", "explain", "give me",
            "tell me", "what", "how", "run"];
        let content_lower = message.content.to_lowercase();
        let should_auto_execute = task_keywords.iter().any(|kw| content_lower.contains(kw));

        if should_auto_execute {
            emit("executing", &format!("Running {} on your data...", agent_def.name), 85);

            // Build context for agent execution
            let agent_context = AgentContext {
                query: Some(message.content.clone()),
                user_info: None,
                space_id: _context.space_id.clone(),
                session_id: _context.conversation_id.clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                conversation_history: _context.conversation_history.clone()
                    .unwrap_or_default()
                    .iter()
                    .map(|msg| ConversationTurn {
                        role: msg.role.clone(),
                        content: msg.content.clone(),
                        timestamp: chrono::Utc::now().timestamp_millis() as u64,
                        metadata: HashMap::new(),
                    })
                    .collect(),
                variables: HashMap::new(),
                metadata: HashMap::new(),
            };

            match agent_system.execute_agent(&agent_def.id, agent_context).await {
                Ok(result) => {
                    emit("complete", "Agent execution complete!", 100);

                    let header = format!(
                        "**{agent_name}** (created & executed in {time}ms)\n\n",
                        agent_name = agent_def.name,
                        time = result.execution_time_ms,
                    );

                    return Ok(AssistantResponse {
                        content: format!("{}{}", header, result.response),
                        artifacts: Vec::new(),
                        citations: Vec::new(),
                        suggestions: vec![
                            "Tell me more".to_string(),
                            format!("Run {} again", agent_def.name),
                            "Create another agent".to_string(),
                        ],
                        search_results: None,
                        metadata: ResponseMetadata {
                            model: Some(format!("agent:{}", agent_def.id)),
                            intent: Intent::AgentCreation,
                            ..Default::default()
                        },
                    });
                }
                Err(e) => {
                    tracing::warn!("Auto-execution failed, returning creation response: {}", e);
                    // Fall through to normal creation response below
                }
            }
        }

        emit("complete", &format!("Agent '{}' created successfully!", agent_def.name), 100);

        let response_content = format!(
            "**Agent Created Successfully!**\n\n\
            **Name:** {}\n\
            **Description:** {}\n\n\
            **Available Tools:** {}\n\
            **Capabilities:** {}\n\n\
            **Configuration:**\n\
            - Temperature: {}\n\
            - Max Tokens: {}\n\n\
            **Status:** Active and ready to use\n\
            **Saved to:** `{}`\n\n\
            Select \"{}\" from the agent dropdown to start using it.",
            agent_def.name,
            agent_def.description,
            tools_str,
            capabilities_str,
            agent_def.config.temperature,
            agent_def.config.max_tokens,
            agent_file.display(),
            agent_def.name
        );

        Ok(AssistantResponse {
            content: response_content,
            artifacts: Vec::new(),
            citations: Vec::new(),
            suggestions: vec![
                format!("Chat with {}", agent_def.name),
                "Create another agent".to_string(),
                "List all agents".to_string(),
            ],
            search_results: None,
            metadata: ResponseMetadata {
                model: Some("agent_creator".to_string()),
                intent: Intent::AgentCreation,
                ..Default::default()
            },
        })
    }

    async fn handle_crew_creation(
        &self,
        message: &UserMessage,
        context: &ChatContext,
        emitter: Option<&dyn EventEmitter>,
    ) -> Result<AssistantResponse> {
        let emit = |stage: &str, msg: &str, progress: u32| {
            if let Some(em) = emitter {
                em.emit(
                    "agent_creation_progress",
                    serde_json::json!({ "stage": stage, "message": msg, "progress": progress }),
                );
            }
        };

        emit("starting", "Designing multi-agent crew...", 10);

        // Immediately stream a visible token so the user doesn't stare at "Thinking..."
        if let Some(em) = emitter {
            let initial = "**Designing crew...** Analyzing your request and creating specialized agents.\n\n";
            em.emit("chat_token", serde_json::json!({
                "token": initial,
                "accumulated": initial,
            }));
        }

        let llm_guard_opt = self.llm_manager.as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not configured"))?;
        let llm_guard = llm_guard_opt.read().await;
        let llm_manager = llm_guard.as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not initialized"))?;

        emit("analyzing", "AI is designing crew roles and workflow...", 30);

        // Ask LLM to design a crew
        let crew_gen_prompt = format!(
            r#"You are an AI crew designer. Based on the user's request, design a crew of 2-4 specialized agents that will collaborate.

User Request: "{}"

Generate a JSON crew definition with this structure:
{{
  "name": "CrewName",
  "description": "What this crew does",
  "process": "sequential",
  "agents": [
    {{
      "name": "AgentName",
      "role": "researcher",
      "goal": "What this agent should achieve",
      "description": "What this agent does",
      "system_prompt": "Detailed system prompt (100-300 words)",
      "capabilities": ["RAGSearch"]
    }},
    {{
      "name": "AgentName2",
      "role": "writer",
      "goal": "What this agent should achieve",
      "description": "What this agent does",
      "system_prompt": "Detailed system prompt",
      "capabilities": ["RAGSearch"]
    }}
  ]
}}

RULES:
- Sequential means agents run in order, each building on previous outputs
- Agent order matters — put the research/data agent first, synthesis/writing last
- Each agent needs a distinct role, goal, and system_prompt
- Use "RAGSearch" capability if the agent needs to search documents
- Return ONLY the JSON object"#,
            message.content
        );

        // Retry once on timeout (Baseten cold starts can exceed initial timeout)
        let llm_response = match llm_manager.generate(&crew_gen_prompt).await {
            Ok(r) => r,
            Err(e) if e.to_string().contains("timed out") || e.to_string().contains("timeout") => {
                tracing::warn!("Crew design LLM call timed out, retrying...");
                if let Some(em) = emitter {
                    em.emit("chat_token", serde_json::json!({
                        "token": "Model warming up, retrying...\n\n",
                        "accumulated": "**Designing crew...** Analyzing your request and creating specialized agents.\n\nModel warming up, retrying...\n\n",
                    }));
                }
                llm_manager.generate(&crew_gen_prompt).await
                    .map_err(|e2| anyhow::anyhow!("Crew design generation failed after retry: {}", e2))?
            }
            Err(e) => return Err(anyhow::anyhow!("Crew design generation failed: {}", e)),
        };

        emit("parsing", "Parsing crew definition...", 50);

        let json_str = llm_response.trim()
            .trim_start_matches("```json").trim_start_matches("```")
            .trim_end_matches("```").trim();

        let crew_json: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse crew JSON: {}", e))?;

        emit("creating", "Creating agents and assembling crew...", 60);

        // Get agent system
        let agent_system_guard = self.agent_system.read().await;
        let agent_system_arc = agent_system_guard.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Agent system not initialized"))?;
        let agent_system = agent_system_arc.write().await;

        // Create each agent and collect their IDs
        let agents_json = crew_json["agents"].as_array()
            .ok_or_else(|| anyhow::anyhow!("No agents array in crew definition"))?;

        let mut crew_members = Vec::new();
        let mut created_agent_names = Vec::new();

        for (idx, agent_json) in agents_json.iter().enumerate() {
            let agent_name = agent_json["name"].as_str().unwrap_or("Agent").to_string();
            let agent_role = agent_json["role"].as_str().unwrap_or("specialist").to_string();
            let agent_goal = agent_json["goal"].as_str().unwrap_or("").to_string();
            let agent_desc = agent_json["description"].as_str().unwrap_or(&agent_name).to_string();
            let system_prompt = agent_json["system_prompt"].as_str().unwrap_or("You are a helpful AI assistant.").to_string();

            let capabilities: Vec<crate::agent::AgentCapability> = agent_json["capabilities"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| {
                    match v.as_str()? {
                        "RAGSearch" => Some(crate::agent::AgentCapability::RAGSearch),
                        "CodeAnalysis" => Some(crate::agent::AgentCapability::CodeAnalysis),
                        "DocumentGeneration" => Some(crate::agent::AgentCapability::DocumentGeneration),
                        _ => None,
                    }
                }).collect())
                .unwrap_or_else(|| vec![crate::agent::AgentCapability::RAGSearch]);

            let agent_def = AgentDefinition {
                id: uuid::Uuid::new_v4().to_string(),
                name: agent_name.clone(),
                description: agent_desc,
                system_prompt,
                config: crate::agent::AgentConfig {
                    auto_use_rag: capabilities.contains(&crate::agent::AgentCapability::RAGSearch),
                    ..Default::default()
                },
                capabilities,
                tools: vec![crate::agent::ToolConfig {
                    tool_id: "rag_search".to_string(),
                    enabled: true,
                    config: HashMap::new(),
                    description: None,
                }],
                enabled: true,
                metadata: HashMap::new(),
            };

            let agent_id = agent_system.register_agent(agent_def).await
                .map_err(|e| anyhow::anyhow!("Failed to register agent '{}': {}", agent_name, e))?;

            crew_members.push(crate::agent::CrewMember {
                agent_id,
                role: agent_role,
                goal: agent_goal,
                order: idx,
            });

            created_agent_names.push(agent_name);
        }

        emit("assembling", "Assembling crew...", 80);

        let crew_name = crew_json["name"].as_str().unwrap_or("AI Crew").to_string();
        let crew_desc = crew_json["description"].as_str().unwrap_or("").to_string();
        let process_str = crew_json["process"].as_str().unwrap_or("sequential");

        let process = if process_str == "hierarchical" {
            let coord_id = crew_members.first().map(|m| m.agent_id.clone()).unwrap_or_default();
            crate::agent::CrewProcess::Hierarchical { coordinator_id: coord_id }
        } else {
            crate::agent::CrewProcess::Sequential
        };

        let crew_def = crate::agent::CrewDefinition {
            id: String::new(),
            name: crew_name.clone(),
            description: crew_desc.clone(),
            agents: crew_members,
            process,
            config: crate::agent::CrewConfig::default(),
        };

        let crew_id = agent_system.register_crew(crew_def).await
            .map_err(|e| anyhow::anyhow!("Failed to register crew: {}", e))?;

        emit("executing", &format!("Running crew '{}'...", crew_name), 85);

        // Stream the crew header so the user sees the team before agents execute
        let agents_summary = created_agent_names.iter().enumerate()
            .map(|(i, name)| format!("{}. {}", i + 1, name))
            .collect::<Vec<_>>().join("\n");

        let header = format!(
            "**Crew '{}' — {} agents, sequential**\n\n**Team:**\n{}\n\n",
            crew_name,
            created_agent_names.len(),
            agents_summary,
        );

        let initial_prefix = "**Designing crew...** Analyzing your request and creating specialized agents.\n\n";
        if let Some(em) = emitter {
            let accumulated = format!("{}{}", initial_prefix, header);
            em.emit("chat_token", serde_json::json!({
                "token": &header,
                "accumulated": &accumulated,
            }));
        }

        // Auto-execute the crew — pass emitter so each agent streams progress
        match agent_system.execute_crew(&crew_id, &message.content, context.space_id.as_deref(), emitter).await {
            Ok(result) => {
                emit("complete", "Crew execution complete!", 100);

                // Build the full content (header was already streamed, agent outputs too)
                let full_content = format!(
                    "{}{}\n\n*Completed in {}ms*",
                    header,
                    result.agent_outputs.iter().enumerate().map(|(i, ao)| {
                        format!(
                            "---\n### Agent {}: {}\n*Role: {} | Goal: —*\n\n{}\n\n",
                            i + 1,
                            ao.agent_name,
                            ao.role,
                            ao.output,
                        )
                    }).collect::<String>(),
                    result.execution_time_ms,
                );

                // Emit chat_complete so frontend finalizes the streamed message
                if let Some(em) = emitter {
                    em.emit("chat_complete", serde_json::json!({ "content": &full_content }));
                }

                return Ok(AssistantResponse {
                    content: full_content,
                    artifacts: Vec::new(),
                    citations: Vec::new(),
                    suggestions: vec![
                        format!("Run crew '{}' again", crew_name),
                        "Create another crew".to_string(),
                        "Tell me more".to_string(),
                    ],
                    search_results: None,
                    metadata: ResponseMetadata {
                        model: Some(format!("crew:{}", crew_id)),
                        intent: Intent::AgentCreation,
                        ..Default::default()
                    },
                });
            }
            Err(e) => {
                tracing::warn!("Crew auto-execution failed: {}", e);
                // Fall through to creation-only response
            }
        }

        emit("complete", &format!("Crew '{}' created!", crew_name), 100);

        let agents_summary = created_agent_names.iter().enumerate()
            .map(|(i, name)| format!("{}. {}", i + 1, name))
            .collect::<Vec<_>>().join("\n");

        Ok(AssistantResponse {
            content: format!(
                "**Crew '{}' Created!**\n\n\
                {}\n\n\
                **Agents:**\n{}\n\n\
                Go to the Agents panel to run this crew, or ask me to run it.",
                crew_name, crew_desc, agents_summary
            ),
            artifacts: Vec::new(),
            citations: Vec::new(),
            suggestions: vec![
                format!("Run crew '{}'", crew_name),
                "Create another crew".to_string(),
            ],
            search_results: None,
            metadata: ResponseMetadata {
                model: Some("crew_creator".to_string()),
                intent: Intent::AgentCreation,
                ..Default::default()
            },
        })
    }

    async fn handle_tool_action(
        &self,
        message: &UserMessage,
        context: &ChatContext,
        emitter: Option<&dyn EventEmitter>,
    ) -> Result<AssistantResponse> {
        let llm_guard_opt = self
            .llm_manager
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not configured"))?;
        let llm_guard = llm_guard_opt.read().await;
        let llm_manager = llm_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not initialized"))?;

        // Build tool schemas from the registry
        let tool_descriptions = self.tool_registry.get_tool_descriptions();
        let tool_schemas = crate::agent::tool_loop::tool_descriptions_to_schemas(&tool_descriptions);

        // Build the system prompt with tool-calling instructions
        let now = chrono::Utc::now();
        let system_prompt = format!(
            "You are a helpful personal assistant with access to tools. \
             Use the provided tools to fulfill the user's request. \
             Today's date is {}. Current time is {} UTC.\n\n\
             When the user asks you to create tasks, events, reminders, or perform \
             any action, use the appropriate tool. Do NOT output code or JSON — \
             call the tool directly.\n\n\
             After executing a tool successfully, provide a brief, friendly confirmation \
             to the user describing what you did.",
            now.format("%Y-%m-%d"),
            now.format("%H:%M"),
        );

        // Build conversation history as ChatMessages
        let mut messages = vec![crate::llm::ChatMessage::system(&system_prompt)];

        if let Some(history) = &context.conversation_history {
            for msg in history.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                match msg.role.as_str() {
                    "user" => messages.push(crate::llm::ChatMessage::user(&msg.content)),
                    "assistant" => messages.push(crate::llm::ChatMessage::assistant(&msg.content)),
                    _ => {}
                }
            }
        }

        messages.push(crate::llm::ChatMessage::user(&message.content));

        // Build agent context for tool execution
        let agent_context = AgentContext {
            query: Some(message.content.clone()),
            conversation_history: Vec::new(),
            variables: HashMap::new(),
            user_info: Some(UserInfo::new("default_user".to_string())),
            space_id: context.space_id.clone(),
            session_id: context
                .conversation_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            metadata: HashMap::new(),
        };

        let loop_config = crate::agent::tool_loop::ToolLoopConfig {
            max_iterations: 5,
            tool_timeout_secs: 30,
            streaming: emitter.is_some(),
        };

        // Bridge EventEmitter to ToolLoopEmitter for streaming
        struct EmitterBridge<'a> {
            inner: &'a dyn EventEmitter,
        }

        impl<'a> crate::agent::tool_loop::ToolLoopEmitter for EmitterBridge<'a> {
            fn on_content_delta(&self, delta: &str) {
                self.inner.emit(
                    "chat_token",
                    serde_json::json!({ "token": delta, "accumulated": "" }),
                );
            }
            fn on_tool_start(&self, tool_name: &str, arguments: &str) {
                self.inner.emit(
                    "tool_execution",
                    serde_json::json!({
                        "stage": "executing",
                        "tool": tool_name,
                        "arguments": arguments,
                    }),
                );
            }
            fn on_tool_complete(&self, invocation: &crate::agent::tool_loop::ToolInvocation) {
                self.inner.emit(
                    "tool_execution",
                    serde_json::json!({
                        "stage": "completed",
                        "tool": invocation.tool_name,
                        "success": invocation.success,
                        "result": invocation.result,
                        "duration_ms": invocation.duration_ms,
                    }),
                );
            }
            fn on_thinking(&self, msg: &str) {
                self.inner.emit(
                    "tool_execution",
                    serde_json::json!({ "stage": "thinking", "message": msg }),
                );
            }
        }

        let bridge = emitter.map(|em| EmitterBridge { inner: em });
        let bridge_ref: Option<&dyn crate::agent::tool_loop::ToolLoopEmitter> =
            bridge.as_ref().map(|b| b as &dyn crate::agent::tool_loop::ToolLoopEmitter);

        let start_time = std::time::Instant::now();
        let result = crate::agent::tool_loop::run_tool_loop(
            llm_manager,
            &self.tool_registry,
            &mut messages,
            &tool_schemas,
            &agent_context,
            &loop_config,
            bridge_ref,
        )
        .await?;
        let duration = start_time.elapsed();

        let model_name = llm_manager
            .info()
            .map(|info| info.model)
            .unwrap_or_else(|| "llm".to_string());

        // Emit chat_complete if streaming
        if let Some(em) = emitter {
            em.emit(
                "chat_complete",
                serde_json::json!({ "content": &result.content }),
            );
        }

        tracing::info!(
            iterations = result.iterations,
            tool_calls = result.tool_invocations.len(),
            duration_ms = duration.as_millis() as u64,
            "Tool action complete"
        );

        Ok(AssistantResponse {
            content: result.content,
            artifacts: Vec::new(),
            citations: Vec::new(),
            suggestions: vec![
                "Show my tasks".to_string(),
                "What's on my calendar?".to_string(),
            ],
            search_results: None,
            metadata: ResponseMetadata {
                model: Some(model_name),
                input_tokens: None,
                output_tokens: None,
                duration_ms: Some(duration.as_millis() as u64),
                intent: Intent::ToolAction,
                ..Default::default()
            },
        })
    }

    async fn handle_general_chat(
        &self,
        message: &UserMessage,
        context: &ChatContext,
    ) -> Result<AssistantResponse> {
        let llm_guard_opt = self
            .llm_manager
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not configured"))?;
        let llm_guard = llm_guard_opt.read().await;
        let llm_manager = llm_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM not initialized"))?;

        let history_text = Self::build_history_text(context);

        let context_window = Self::get_context_window_from_llm(llm_manager);
        let available_for_history = context_window
            .saturating_sub(1500)
            .saturating_sub(2000)
            .saturating_sub(estimate_tokens(&message.content) + 100);
        let history_text = Self::truncate_to_budget(&history_text, available_for_history);

        let general_instructions = context.custom_system_prompt.as_ref()
            .map(|custom| format!("{}\n\n{}", custom, GENERAL_CHAT_PROMPT))
            .unwrap_or_else(|| GENERAL_CHAT_PROMPT.to_string());
        let prompt = format!(
            "{}\n{}\n{}User: {}\n\nAssistant:",
            general_instructions, STRUCTURED_OUTPUT_INSTRUCTIONS, history_text, message.content
        );

        let start_time = std::time::Instant::now();
        let response = llm_manager
            .generate(&prompt)
            .await
            .map_err(|e| anyhow::anyhow!("LLM generation failed: {}", e))?;
        let duration = start_time.elapsed();

        Ok(AssistantResponse {
            content: response.clone(),
            artifacts: Vec::new(),
            citations: Vec::new(),
            suggestions: vec![
                "Tell me more".to_string(),
                "Can you explain that differently?".to_string(),
            ],
            search_results: None,
            metadata: ResponseMetadata {
                model: Some("llm".to_string()),
                input_tokens: Some(estimate_tokens(&prompt)),
                output_tokens: Some(estimate_tokens(&response)),
                duration_ms: Some(duration.as_millis() as u64),
                intent: Intent::General,
                ..Default::default()
            },
        })
    }

    // ========================================================================
    // Memory Integration
    // ========================================================================

    async fn retrieve_relevant_memories(&self, message: &UserMessage) -> Result<Vec<Memory>> {
        let memory_system = self.memory.read().await;
        let time_range = Some((Utc::now() - chrono::Duration::days(7), Utc::now()));

        let query = Query {
            query_text: Some(message.content.clone()),
            query_embedding: None,
            time_range,
            experience_types: Some(vec![
                ExperienceType::Conversation,
                ExperienceType::Search,
                ExperienceType::Context,
                ExperienceType::Decision,
            ]),
            importance_threshold: Some(0.5),
            max_results: 5,
            retrieval_mode: RetrievalMode::Temporal,
        };

        Ok(memory_system.retrieve(&query).unwrap_or_default())
    }

    async fn store_conversation_memory(
        &self,
        message: &UserMessage,
        response: &AssistantResponse,
        context: &ChatContext,
    ) -> Result<()> {
        let mut memory_system = self.memory.write().await;
        let now = Utc::now();
        let hour = now.hour();
        let time_of_day = if hour < 6 {
            "night"
        } else if hour < 12 {
            "morning"
        } else if hour < 18 {
            "afternoon"
        } else {
            "evening"
        };

        let entities = Self::extract_entities(&message.content);

        let rich_context = RichContext {
            id: ContextId(Uuid::new_v4()),
            conversation: MemConversationContext {
                conversation_id: context.conversation_id.clone(),
                topic: Self::extract_topic(&message.content),
                recent_messages: vec![message.content.clone()],
                mentioned_entities: entities.clone(),
                active_intents: vec![format!("{:?}", response.metadata.intent)],
                tone: None,
            },
            user: UserContext {
                user_id: None,
                name: None,
                preferences: HashMap::new(),
                work_patterns: Vec::new(),
                expertise: Vec::new(),
                goals: Vec::new(),
                learning_style: None,
            },
            project: ProjectContext {
                project_id: context.project.clone(),
                name: context.project.clone(),
                project_type: None,
                technologies: Vec::new(),
                current_phase: None,
                active_files: Vec::new(),
                current_task: None,
                dependencies: Vec::new(),
            },
            temporal: TemporalContext {
                time_of_day: Some(time_of_day.to_string()),
                day_of_week: Some(format!("{}", now.weekday())),
                session_duration_minutes: None,
                time_since_last_interaction: None,
                patterns: Vec::new(),
                trends: Vec::new(),
            },
            semantic: SemanticContext {
                concepts: Self::extract_concepts(&message.content),
                related_concepts: Vec::new(),
                relationships: Vec::new(),
                domain: None,
                abstraction_level: None,
                tags: Self::extract_keywords(&message.content),
            },
            code: CodeContext::default(),
            document: DocumentContext {
                document_id: None,
                document_type: None,
                current_section: None,
                related_documents: Vec::new(),
                citations: response.citations.iter().map(|c| c.title.clone()).collect(),
                categories: Vec::new(),
            },
            environment: EnvironmentContext {
                os: Some(std::env::consts::OS.to_string()),
                device: Some("desktop".to_string()),
                screen_size: None,
                location: None,
                network: None,
                resources: HashMap::new(),
            },
            parent: None,
            embeddings: None,
            decay_rate: 0.95,
            created_at: now,
            updated_at: now,
        };

        // Store user message
        let user_exp = Experience {
            experience_type: ExperienceType::Conversation,
            content: format!("User: {}", message.content),
            context: Some(rich_context.clone()),
            entities: entities.clone(),
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("role".to_string(), "user".to_string());
                meta.insert("platform".to_string(), format!("{:?}", message.platform));
                if let Some(ref sid) = context.space_id {
                    meta.insert("space_id".to_string(), sid.clone());
                }
                if let Some(ref cid) = context.conversation_id {
                    meta.insert("conversation_id".to_string(), cid.clone());
                }
                meta
            },
            embeddings: None,
            related_memories: Vec::new(),
            causal_chain: Vec::new(),
            outcomes: Vec::new(),
        };
        memory_system.record(user_exp).ok();

        // Store assistant response
        let mut response_context = rich_context;
        response_context.id = ContextId(Uuid::new_v4());
        response_context.semantic.concepts = Self::extract_concepts(&response.content);
        response_context.semantic.tags = Self::extract_keywords(&response.content);

        let assistant_exp = Experience {
            experience_type: ExperienceType::Conversation,
            content: format!("Assistant: {}", response.content),
            context: Some(response_context),
            entities,
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("role".to_string(), "assistant".to_string());
                meta.insert(
                    "intent".to_string(),
                    format!("{:?}", response.metadata.intent),
                );
                if let Some(ref model) = response.metadata.model {
                    meta.insert("model".to_string(), model.clone());
                }
                meta
            },
            embeddings: None,
            related_memories: Vec::new(),
            causal_chain: Vec::new(),
            outcomes: Vec::new(),
        };
        memory_system.record(assistant_exp).ok();

        Ok(())
    }

    // ========================================================================
    // Helpers
    // ========================================================================

    fn build_conversation_context(context: &ChatContext) -> RagConversationContext {
        let history = context.conversation_history.as_ref();

        // Extract recent messages (last 6 turns for context)
        let recent_messages: Vec<String> = history
            .map(|h| {
                h.iter()
                    .rev()
                    .take(6)
                    .map(|m| format!("{}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();

        // Extract entities: capitalized multi-word names, proper nouns from conversation
        let mut entities: Vec<String> = Vec::new();
        let mut seen_entities: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Some(h) = history {
            for msg in h.iter().rev().take(10) {
                // Extract capitalized words/phrases (likely names, places, organizations)
                let words: Vec<&str> = msg.content.split_whitespace().collect();
                let mut i = 0;
                while i < words.len() {
                    let word = words[i].trim_matches(|c: char| !c.is_alphanumeric());
                    if word.len() > 1
                        && word.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                        && !Self::is_common_sentence_starter(word)
                    {
                        // Try to capture multi-word names (e.g., "Anushree Sharma")
                        let mut name_parts = vec![word.to_string()];
                        let mut j = i + 1;
                        while j < words.len() {
                            let next = words[j].trim_matches(|c: char| !c.is_alphanumeric());
                            if next.len() > 1
                                && next.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                                && !Self::is_common_sentence_starter(next)
                            {
                                name_parts.push(next.to_string());
                                j += 1;
                            } else {
                                break;
                            }
                        }

                        let entity = name_parts.join(" ");
                        let key = entity.to_lowercase();
                        if !seen_entities.contains(&key) {
                            seen_entities.insert(key);
                            entities.push(entity);
                        }
                        i = j;
                    } else {
                        i += 1;
                    }
                }
            }
        }

        // Extract files discussed
        let files_discussed: Vec<String> = history
            .map(|h| {
                let mut files = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for msg in h.iter() {
                    for word in msg.content.split_whitespace() {
                        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '/' && c != '\\');
                        if clean.contains('.')
                            && clean.len() > 4
                            && !clean.starts_with("http")
                            && !clean.starts_with("www.")
                        {
                            let key = clean.to_lowercase();
                            if !seen.contains(&key) {
                                seen.insert(key);
                                files.push(clean.to_string());
                            }
                        }
                    }
                }
                files
            })
            .unwrap_or_default();

        // Infer topic from the first substantive user message in recent history
        let topic = history
            .and_then(|h| {
                h.iter()
                    .filter(|m| m.role == "user" && m.content.split_whitespace().count() > 3)
                    .last()
                    .map(|m| {
                        // Use first 8 content words as topic summary
                        m.content
                            .split_whitespace()
                            .take(8)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
            })
            .unwrap_or_default();

        // Extract key concepts: domain-specific terms that appear in assistant responses
        // (terms the system has already discussed are likely relevant)
        let concepts_mentioned: Vec<String> = history
            .map(|h| {
                let mut concept_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                for msg in h.iter().filter(|m| m.role == "assistant") {
                    for word in msg.content.split_whitespace() {
                        let clean = word
                            .trim_matches(|c: char| !c.is_alphanumeric())
                            .to_lowercase();
                        if clean.len() > 4
                            && !Self::is_stop_word(&clean)
                        {
                            *concept_counts.entry(clean).or_insert(0) += 1;
                        }
                    }
                }
                let mut concepts: Vec<(String, usize)> = concept_counts.into_iter().collect();
                concepts.sort_by(|a, b| b.1.cmp(&a.1));
                concepts.into_iter().take(10).map(|(c, _)| c).collect()
            })
            .unwrap_or_default();

        RagConversationContext {
            topic,
            recent_messages,
            concepts_mentioned,
            files_discussed,
            entities,
        }
    }

    /// Words that commonly start sentences but aren't entity names
    fn is_common_sentence_starter(word: &str) -> bool {
        matches!(
            word,
            "The" | "This" | "That" | "These" | "Those" | "What" | "Where" | "When"
            | "How" | "Why" | "Who" | "Which" | "Can" | "Could" | "Would" | "Should"
            | "Will" | "Do" | "Does" | "Did" | "Is" | "Are" | "Was" | "Were" | "Have"
            | "Has" | "Had" | "It" | "If" | "In" | "On" | "At" | "To" | "For" | "But"
            | "And" | "Or" | "Not" | "Yes" | "No" | "Found" | "Based" | "According"
            | "Here" | "There" | "Some" | "Any" | "All" | "Each" | "Every" | "My"
            | "Your" | "His" | "Her" | "Its" | "Our" | "Their" | "From" | "With"
            | "About" | "After" | "Before" | "Between" | "During" | "Since" | "Until"
            | "Sure" | "Thanks" | "Thank" | "Please" | "Sorry" | "Let" | "Try"
            | "Show" | "Tell" | "Give" | "Also" | "However" | "Moreover" | "Furthermore"
        )
    }

    fn is_stop_word(word: &str) -> bool {
        matches!(
            word,
            "the" | "this" | "that" | "these" | "those" | "what" | "where"
            | "when" | "how" | "why" | "who" | "which" | "have" | "has" | "had"
            | "been" | "being" | "will" | "would" | "could" | "should" | "about"
            | "with" | "from" | "into" | "through" | "during" | "before" | "after"
            | "above" | "below" | "between" | "under" | "again" | "further" | "then"
            | "once" | "here" | "there" | "some" | "other" | "more" | "most" | "very"
            | "just" | "also" | "than" | "each" | "every" | "both" | "does" | "doing"
            | "their" | "them" | "they" | "your" | "yours" | "information" | "based"
            | "found" | "following" | "according" | "contains" | "including" | "provide"
            | "provided" | "shows" | "shown" | "document" | "documents" | "context"
        )
    }

    fn build_history_text(context: &ChatContext) -> String {
        if let Some(history) = &context.conversation_history {
            if !history.is_empty() {
                let messages: Vec<(String, String)> = history
                    .iter()
                    .map(|msg| (msg.role.clone(), msg.content.clone()))
                    .collect();
                let compressed = compress_history(&messages, 5);
                return format_compressed_history(&compressed);
            }
        }
        String::new()
    }

    fn build_memory_text(memories: &[Memory]) -> String {
        if memories.is_empty() {
            return String::new();
        }
        let formatted = memories
            .iter()
            .map(|mem| {
                let time_ago = Utc::now()
                    .signed_duration_since(mem.created_at)
                    .num_minutes();
                format!(
                    "[{}min ago, importance: {:.2}] {}",
                    time_ago, mem.importance, mem.experience.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\n\nConversation Memory (for topic continuity ONLY — do NOT use as a source of facts):\n{}\n",
            formatted
        )
    }

    fn get_context_window_from_llm(llm_manager: &LLMManager) -> usize {
        llm_manager
            .info()
            .and_then(|info| {
                if info.context_window > 0 {
                    Some(info.context_window)
                } else {
                    None
                }
            })
            .unwrap_or(8192)
    }

    fn truncate_context_to_budget(context_text: &str, max_tokens: usize) -> String {
        let current = estimate_tokens(context_text);
        if current <= max_tokens {
            return context_text.to_string();
        }

        let chunks: Vec<&str> = context_text.split("\n\n").collect();
        let mut result = String::new();
        let mut used = 0;

        for chunk in chunks {
            let t = estimate_tokens(chunk);
            if used + t > max_tokens {
                break;
            }
            if !result.is_empty() {
                result.push_str("\n\n");
                used += 1;
            }
            result.push_str(chunk);
            used += t;
        }

        if result.is_empty() {
            let max_chars = max_tokens * 4;
            let mut end = max_chars.min(context_text.len());
            while end > 0 && !context_text.is_char_boundary(end) {
                end -= 1;
            }
            return context_text[..end].to_string();
        }

        result
    }

    fn truncate_to_budget(text: &str, max_tokens: usize) -> String {
        let current = estimate_tokens(text);
        if current <= max_tokens {
            return text.to_string();
        }

        let lines: Vec<&str> = text.lines().collect();
        let mut result_lines: Vec<&str> = Vec::new();
        let mut used = 0;

        for line in lines.iter().rev() {
            let t = estimate_tokens(line);
            if used + t > max_tokens {
                break;
            }
            result_lines.push(line);
            used += t;
        }

        result_lines.reverse();
        result_lines.join("\n")
    }

    /// Remove near-duplicate chunks by comparing word overlap.
    /// Two chunks sharing >60% of their words are considered duplicates;
    /// only the higher-scored one survives.
    fn deduplicate_by_content(mut results: Vec<SearchResult>) -> Vec<SearchResult> {
        if results.len() <= 1 {
            return results;
        }

        // Pre-compute word sets for each chunk (lowercase, alphanumeric only)
        let word_sets: Vec<std::collections::HashSet<String>> = results
            .iter()
            .map(|r| {
                r.text
                    .split_whitespace()
                    .map(|w| w.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>())
                    .filter(|w| w.len() > 2)
                    .collect()
            })
            .collect();

        let mut keep = vec![true; results.len()];

        // Results are already sorted by score (descending from reranker/merge).
        // Walk top-down: for each kept chunk, mark later chunks as duplicates
        // if they share >60% word overlap.
        for i in 0..results.len() {
            if !keep[i] {
                continue;
            }
            for j in (i + 1)..results.len() {
                if !keep[j] {
                    continue;
                }
                let overlap = Self::jaccard_similarity(&word_sets[i], &word_sets[j]);
                if overlap > 0.60 {
                    keep[j] = false;
                    tracing::debug!(
                        kept_score = results[i].score,
                        dropped_score = results[j].score,
                        overlap = format!("{:.0}%", overlap * 100.0),
                        "Dedup: dropped near-duplicate chunk"
                    );
                }
            }
        }

        let mut idx = 0;
        results.retain(|_| {
            let k = keep[idx];
            idx += 1;
            k
        });
        results
    }

    fn jaccard_similarity(
        a: &std::collections::HashSet<String>,
        b: &std::collections::HashSet<String>,
    ) -> f64 {
        if a.is_empty() && b.is_empty() {
            return 1.0;
        }
        let intersection = a.intersection(b).count();
        let union = a.union(b).count();
        if union == 0 {
            return 0.0;
        }
        intersection as f64 / union as f64
    }

    /// Cut off chunks after a sharp relevance drop.
    /// If chunk[i+1].score < chunk[i].score * 0.6 (a 40%+ relative drop),
    /// everything from i+1 onward is discarded — it's below the relevance cliff.
    /// Always keeps at least the top 2 chunks so the LLM has something to compare.
    fn cut_at_score_cliff(results: Vec<SearchResult>) -> Vec<SearchResult> {
        if results.len() <= 2 {
            return results;
        }

        let mut cut_at = results.len();
        for i in 1..results.len() {
            let prev = results[i - 1].score;
            let curr = results[i].score;
            // 40% relative drop from previous chunk
            if prev > 0.0 && curr < prev * 0.6 && i >= 2 {
                tracing::debug!(
                    position = i,
                    prev_score = prev,
                    curr_score = curr,
                    drop_pct = format!("{:.0}%", (1.0 - curr / prev) * 100.0),
                    "Score cliff detected — trimming remaining chunks"
                );
                cut_at = i;
                break;
            }
        }

        results.into_iter().take(cut_at).collect()
    }

    /// Merge results from multiple query variants, deduplicating by chunk ID
    /// and keeping the highest score for each unique chunk.
    fn merge_expanded_results(
        result_sets: Vec<Vec<crate::types::SimpleSearchResult>>,
        limit: usize,
    ) -> Vec<crate::types::SimpleSearchResult> {
        use std::collections::HashMap;

        if result_sets.is_empty() {
            return Vec::new();
        }
        if result_sets.len() == 1 {
            let mut single = result_sets.into_iter().next().unwrap();
            single.truncate(limit);
            return single;
        }

        // Deduplicate by chunk ID, keeping the highest-scoring version
        let mut best_by_id: HashMap<String, crate::types::SimpleSearchResult> = HashMap::new();

        // Interleave results round-robin to ensure each variant contributes
        let max_len = result_sets.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut ordered_ids: Vec<String> = Vec::new();

        for idx in 0..max_len {
            for result_set in &result_sets {
                if idx < result_set.len() {
                    let r = &result_set[idx];
                    let key = r.id.to_string();

                    match best_by_id.get(&key) {
                        Some(existing) if existing.score >= r.score => {}
                        _ => {
                            if !best_by_id.contains_key(&key) {
                                ordered_ids.push(key.clone());
                            }
                            best_by_id.insert(key, r.clone());
                        }
                    }
                }
            }
        }

        // Collect in interleaved order, then sort by score
        let mut merged: Vec<crate::types::SimpleSearchResult> = ordered_ids
            .into_iter()
            .filter_map(|id| best_by_id.remove(&id))
            .collect();

        merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        merged.truncate(limit);
        merged
    }

    fn format_fallback_results(search_results: &[SearchResult]) -> String {
        format!(
            "Found {} relevant results:\n\n{}",
            search_results.len(),
            search_results
                .iter()
                .take(5)
                .map(|r| format!("- {}", r.text.chars().take(200).collect::<String>()))
                .collect::<Vec<_>>()
                .join("\n\n")
        )
    }

    fn extract_topic(content: &str) -> Option<String> {
        let words: Vec<&str> = content.split_whitespace().take(5).collect();
        if words.is_empty() {
            None
        } else {
            Some(words.join(" "))
        }
    }

    fn extract_concepts(text: &str) -> Vec<String> {
        text.split_whitespace()
            .filter(|w| w.len() > 5)
            .take(10)
            .map(|w| w.to_lowercase())
            .collect()
    }

    fn extract_keywords(text: &str) -> Vec<String> {
        text.split_whitespace()
            .filter(|w| w.len() > 3 && !w.chars().all(|c| c.is_ascii_punctuation()))
            .take(15)
            .map(|w| w.to_lowercase())
            .collect()
    }

    fn extract_entities(text: &str) -> Vec<String> {
        let mut entities = Vec::new();
        for word in text.split_whitespace() {
            if word.len() > 2
                && word
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                let cleaned = word.trim_matches(|c: char| !c.is_alphanumeric());
                if !cleaned.is_empty() {
                    entities.push(cleaned.to_string());
                }
            }
        }
        entities.truncate(20);
        entities
    }
}
