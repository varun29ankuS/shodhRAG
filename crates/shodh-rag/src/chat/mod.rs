pub mod engine;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

// Pre-compiled regexes — compiled once, reused on every call.
static ARTIFACT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)<artifact[^>]*\s+(?:id|identifier)="([^"]+)"[^>]*\s+type="([^"]+)"[^>]*(?:\s+language="([^"]+)")?[^>]*\s+title="([^"]+)"[^>]*>(.*?)</artifact>"#
    ).expect("artifact regex is valid")
});
static STRIP_ARTIFACT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)<artifact[^>]*>.*?</artifact>").expect("strip artifact regex is valid")
});
static CITATION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[(\d+)\]").expect("citation regex is valid"));

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: String,
    pub images: Option<Vec<Vec<u8>>>,
    pub platform: MessagePlatform,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagePlatform {
    Desktop,
    WhatsApp,
    Telegram,
    Discord,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatContext {
    pub agent_id: Option<String>,
    pub project: Option<String>,
    pub space_id: Option<String>,
    pub conversation_id: Option<String>,
    pub conversation_history: Option<Vec<ConversationMessage>>,
    pub max_results: Option<usize>,
    pub streaming: Option<bool>,
    pub custom_system_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantResponse {
    pub content: String,
    pub artifacts: Vec<Artifact>,
    pub citations: Vec<Citation>,
    pub suggestions: Vec<String>,
    pub search_results: Option<Vec<SearchResult>>,
    pub metadata: ResponseMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,
    pub language: Option<String>,
    pub editable: bool,
    pub version: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactType {
    Code,
    Markdown,
    Mermaid,
    Table,
    Chart,
    Html,
    Svg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub title: String,
    pub snippet: String,
    pub score: f32,
    pub url: Option<String>,
    pub authors: Vec<String>,
    pub source: String,
    pub year: String,
    pub page_numbers: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub text: String,
    pub score: f32,
    pub citation: Option<Citation>,
    pub source_file: String,
    pub page_number: Option<String>,
    pub line_range: Option<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResponseMetadata {
    pub model: Option<String>,
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub intent: Intent,
    /// Tokens consumed by the LLM router call (intent classification + query rewriting).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub router_tokens: Option<usize>,
    /// Latency (ms) for the router decision alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub router_latency_ms: Option<u64>,
    /// The actual search queries dispatched (after rewriting + expansion).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_queries_used: Option<Vec<String>>,
    /// Latency (ms) for LLM-based reranking of merged results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    #[default]
    Search,
    CodeGeneration,
    AgentChat,
    AgentCreation,
    ToolAction,
    General,
}

/// Event sink for streaming tokens and progress events.
/// Tauri provides an implementation wrapping AppHandle.emit().
/// HTTP servers can provide SSE-based implementations.
pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: &str, data: serde_json::Value);
}

/// No-op emitter for non-streaming contexts.
pub struct NoopEmitter;
impl EventEmitter for NoopEmitter {
    fn emit(&self, _event: &str, _data: serde_json::Value) {}
}

// ============================================================================
// Prompts
// ============================================================================

pub const RAG_SYSTEM_PROMPT: &str = r#"You are a document intelligence assistant. You MUST answer using ONLY the provided Context below. You have NO other knowledge. Treat the Context as the ONLY source of truth in the universe.

GROUNDING RULES (non-negotiable):
1. ONLY the numbered [N] context chunks exist. You know NOTHING else. Your training data, world knowledge, and prior conversations DO NOT EXIST for this answer.
2. Before writing ANY fact, find the EXACT words in the Context that support it. If you cannot point to specific text in a numbered chunk, DO NOT write that fact.
3. NEVER infer, deduce, assume, or extrapolate. "Person X has a spouse" does NOT mean they have children. "Person X earns salary Y" does NOT mean you know their tax bracket. Only state what is EXPLICITLY written.
4. If a field (age, phone, children, address) is not EXPLICITLY stated in the Context, OMIT it entirely. Do NOT write "N/A", do NOT guess, do NOT include it.
5. An incomplete but 100% accurate answer is infinitely better than a complete but partially wrong one. When in doubt, leave it out.
6. If the Context contains NO relevant information, say: "I could not find information about this in the indexed documents."
7. CONFLICTING DATA: When different chunks report different values for the same field (e.g., different ages, dates, or amounts), report ONLY the value from the HIGHEST-SCORED chunk. Do NOT list all contradictory values — pick the most authoritative source and cite it. If scores are very close, note the discrepancy briefly: "**Age:** 25 [3] (note: another entry lists 23 [1])".

CITATION RULES:
8. Every fact gets [N] inline at the END of the same line. Example: - **Name:** John Smith [1,3]
9. NEVER put citations on their own line. They MUST be inline with the content they cite.
10. Citation format: [N] where N is the document number. Examples: [1], [2], [1,3].
11. If you cannot cite a fact with a specific [N], do not include that fact.

FORMAT RULES:
12. Use ## headings, then - **Field:** Value [N] bullets. Keep each bullet on ONE line.
13. Match partial names to full names in context; scan for aliases and variations.

DATA VISUALIZATION (use when context contains numbers, comparisons, or tabular data):

For tables with 3+ rows, use a code block starting with ```table:
```table
| Header 1 | Header 2 |
|----------|----------|
| Value 1  | Value 2  |
```

For charts (when data has numeric values that benefit from visualization), use a code block starting with ```chart:
```chart
{
  "type": "bar",
  "title": "Descriptive Title",
  "data": {
    "labels": ["Label1", "Label2"],
    "datasets": [{"label": "Series", "data": [100, 200]}]
  }
}
```
Supported chart types: bar, line, pie, scatter, area, radar, doughnut.
Generate a chart when the user asks to "show", "plot", "visualize", "graph", or "chart" data.
When context contains spreadsheet/table data with numeric columns, offer a chart alongside the textual answer.
"#;

pub const CODE_GENERATION_PROMPT: &str = r#"You are an expert code generator with artifact generation capabilities.

## Artifact Format
Wrap ALL code snippets (5+ lines) in artifact tags using this EXACT format:

<artifact id="unique-id" type="code" language="python|javascript|rust|..." title="Descriptive Title">
// Your code here (NO markdown code fences inside artifacts)
</artifact>

## Code Requirements:
- Production-grade only (NO TODOs, placeholders, or mocks)
- Clear comments
- Error handling
- Type annotations
- Best practices

Explain your implementation briefly BEFORE the artifact."#;

pub const GENERAL_CHAT_PROMPT: &str = r#"You are a helpful AI assistant with the ability to create artifacts.

## Artifact Guidelines

When generating substantial, reusable content (code snippets, diagrams, documents), wrap it in artifact tags:

<artifact id="unique-id" type="code|markdown|mermaid|html|svg" language="python|javascript|rust|..." title="Descriptive Title">
content here
</artifact>

**When to use artifacts:**
- Code snippets (5+ lines)
- Mermaid diagrams (flowcharts, sequence diagrams, etc.)
- Markdown documents
- HTML/SVG visualizations

**Artifact Types:**
- `type="code"` + `language="..."` - Code in any language
- `type="mermaid"` + `language="mermaid"` - Mermaid diagrams
- `type="markdown"` - Formatted documents
- `type="html"` - HTML content
- `type="svg"` - SVG graphics

For explanations or short code snippets (< 5 lines), just write them normally without artifact tags.
"#;

// ============================================================================
// Utility Functions (artifacts, formatting, citations)
// ============================================================================

/// Extract artifacts from LLM response text.
/// Extract artifacts from LLM response content and return cleaned content
/// with artifact blocks removed. Handles both `<artifact>` XML tags and
/// standalone code blocks (```mermaid, ```flowchart, ```code, etc.)
pub fn extract_artifacts(content: &str) -> (Vec<Artifact>, String) {
    let mut artifacts = Vec::new();

    for cap in ARTIFACT_RE.captures_iter(content) {
        let id = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let type_str = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let language = cap.get(3).map(|m| m.as_str().to_string());
        let title = cap.get(4).map(|m| m.as_str()).unwrap_or("");
        let artifact_content = cap.get(5).map(|m| m.as_str()).unwrap_or("");

        let artifact_type = match type_str {
            "code" => ArtifactType::Code,
            "markdown" => ArtifactType::Markdown,
            "mermaid" => ArtifactType::Mermaid,
            "table" => ArtifactType::Table,
            "chart" => ArtifactType::Chart,
            "html" => ArtifactType::Html,
            "svg" => ArtifactType::Svg,
            _ => continue,
        };

        let clean_content = if artifact_content.trim().starts_with("```") {
            artifact_content
                .trim()
                .strip_prefix("```")
                .and_then(|s| s.split_once('\n').map(|(_, rest)| rest))
                .and_then(|s| s.strip_suffix("```"))
                .unwrap_or(artifact_content)
                .trim()
                .to_string()
        } else {
            artifact_content.trim().to_string()
        };

        artifacts.push(Artifact {
            id: id.to_string(),
            artifact_type,
            title: title.to_string(),
            content: clean_content,
            language,
            editable: true,
            version: 1,
            created_at: Utc::now(),
        });
    }

    // If XML artifacts were found, strip them and return early
    if !artifacts.is_empty() {
        let cleaned = strip_artifact_tags(content);
        return (artifacts, cleaned);
    }

    // Fallback: detect standalone code blocks
    {
        let mut idx = 0;
        let mut pos = 0;
        let mut block_ranges: Vec<(usize, usize)> = Vec::new();

        while let Some(start_pos) = content[pos..].find("```") {
            let abs_start = pos + start_pos;
            let after_ticks = abs_start + 3;

            if let Some(newline_pos) = content[after_ticks..].find('\n') {
                let lang_end = after_ticks + newline_pos;
                let language_opt = if newline_pos > 0 {
                    let lang = content[after_ticks..lang_end].trim();
                    if !lang.is_empty() {
                        Some(lang)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let content_start = lang_end + 1;

                if let Some(end_offset) = content[content_start..].find("\n```") {
                    let content_end = content_start + end_offset;
                    let block_end = content_end + 4; // includes "\n```"
                    let code_content = &content[content_start..content_end];

                    let is_mermaid = matches!(
                        language_opt,
                        Some(
                            "mermaid"
                                | "flowchart"
                                | "sequence"
                                | "class"
                                | "erdiagram"
                                | "er"
                                | "state"
                                | "gantt"
                                | "gitgraph"
                                | "git"
                                | "journey"
                        )
                    );

                    let is_table = language_opt == Some("table");
                    let is_chart = language_opt == Some("chart");

                    if is_mermaid {
                        let title = match language_opt.unwrap_or("mermaid") {
                            "flowchart" => "Flowchart",
                            "sequence" => "Sequence Diagram",
                            "class" => "Class Diagram",
                            "erdiagram" | "er" => "ER Diagram",
                            "state" => "State Diagram",
                            "gantt" => "Gantt Chart",
                            "gitgraph" | "git" => "Git Graph",
                            "journey" => "User Journey",
                            _ => "Mermaid Diagram",
                        };
                        artifacts.push(Artifact {
                            id: format!("mermaid-{}", idx),
                            artifact_type: ArtifactType::Mermaid,
                            title: title.to_string(),
                            content: code_content.trim().to_string(),
                            language: None,
                            editable: true,
                            version: 1,
                            created_at: Utc::now(),
                        });
                        block_ranges.push((abs_start, block_end));
                        idx += 1;
                    } else if is_table {
                        let table_title = code_content
                            .lines()
                            .next()
                            .filter(|line| line.contains('|'))
                            .map(|line| {
                                let cols: Vec<&str> = line
                                    .split('|')
                                    .map(|s| s.trim())
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                if cols.len() <= 3 {
                                    cols.join(" / ")
                                } else {
                                    format!("{} (+{} cols)", cols[..2].join(" / "), cols.len() - 2)
                                }
                            })
                            .unwrap_or_else(|| format!("Table {}", idx + 1));
                        artifacts.push(Artifact {
                            id: format!("table-{}", idx),
                            artifact_type: ArtifactType::Table,
                            title: table_title,
                            content: code_content.trim().to_string(),
                            language: Some("table".to_string()),
                            editable: true,
                            version: 1,
                            created_at: Utc::now(),
                        });
                        block_ranges.push((abs_start, block_end));
                        idx += 1;
                    } else if is_chart {
                        let chart_title =
                            serde_json::from_str::<serde_json::Value>(code_content.trim())
                                .ok()
                                .and_then(|v| {
                                    v.get("title").and_then(|t| t.as_str()).map(String::from)
                                })
                                .unwrap_or_else(|| format!("Chart {}", idx + 1));
                        artifacts.push(Artifact {
                            id: format!("chart-{}", idx),
                            artifact_type: ArtifactType::Chart,
                            title: chart_title,
                            content: code_content.trim().to_string(),
                            language: Some("chart".to_string()),
                            editable: true,
                            version: 1,
                            created_at: Utc::now(),
                        });
                        block_ranges.push((abs_start, block_end));
                        idx += 1;
                    } else if code_content.lines().count() > 5 {
                        artifacts.push(Artifact {
                            id: format!("code-{}", idx),
                            artifact_type: ArtifactType::Code,
                            title: format!("Code snippet {}", idx + 1),
                            content: code_content.trim().to_string(),
                            language: language_opt.map(|s| s.to_string()),
                            editable: true,
                            version: 1,
                            created_at: Utc::now(),
                        });
                        block_ranges.push((abs_start, block_end));
                        idx += 1;
                    }

                    pos = block_end;
                } else {
                    pos = after_ticks;
                }
            } else {
                pos = after_ticks;
            }
        }

        // Build cleaned content with extracted code blocks removed
        if !artifacts.is_empty() && !block_ranges.is_empty() {
            let mut cleaned = String::with_capacity(content.len());
            let mut last_end = 0;
            for (start, end) in &block_ranges {
                cleaned.push_str(&content[last_end..*start]);
                last_end = *end;
            }
            cleaned.push_str(&content[last_end..]);
            return (artifacts, cleaned.trim().to_string());
        }
    }

    // No artifacts found — return content as-is
    (artifacts, content.to_string())
}

/// Strip artifact tags from content for display.
pub fn strip_artifact_tags(content: &str) -> String {
    STRIP_ARTIFACT_RE
        .replace_all(content, "")
        .trim()
        .to_string()
}

/// Validate citations — strip references to non-existent sources.
pub fn validate_citations(response: &str, num_sources: usize) -> String {
    let mut result = response.to_string();
    let invalid: Vec<String> = CITATION_RE
        .captures_iter(response)
        .filter_map(|cap| {
            if let Ok(n) = cap[1].parse::<usize>() {
                if n > num_sources || n == 0 {
                    return Some(cap[0].to_string());
                }
            }
            None
        })
        .collect();
    for inv in &invalid {
        result = result.replace(inv, "");
    }
    result
}

/// Force bullet point formatting when LLM returns wall-of-text.
/// Only triggers when the content has no structure (no headers, no bullets, no newlines).
pub fn force_bullet_format(content: &str) -> String {
    // Already structured — don't touch it
    let has_headers = content.contains("## ") || content.contains("# ");
    let has_bullets =
        content.contains("\n- ") || content.contains("\n* ") || content.contains("\n1.");
    let has_paragraphs = content.matches("\n\n").count() >= 2;

    if has_headers || has_bullets || has_paragraphs {
        return content.replace("\n\n\n", "\n\n");
    }

    // Split into sentences and convert to bullet list
    let sentences: Vec<&str> = content
        .split(&['.', '!', '?'][..])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && s.len() > 10)
        .collect();

    if sentences.is_empty() {
        return content.to_string();
    }

    let mut formatted = String::new();
    for sentence in &sentences {
        if sentence
            .chars()
            .all(|c| c.is_whitespace() || c == '[' || c == ']' || c.is_numeric() || c == ',')
        {
            continue;
        }
        formatted.push_str(&format!("- {}.\n", sentence.trim()));
    }

    if formatted.is_empty() {
        return content.to_string();
    }

    formatted
}

/// Build corpus statistics from the RAG engine for a given space.
/// Single source of truth — used by both chat engine and retrieval commands.
pub async fn build_corpus_stats(
    rag: &crate::rag_engine::RAGEngine,
    space_id: Option<&str>,
) -> anyhow::Result<crate::rag::CorpusStats> {
    use crate::types::MetadataFilter;
    use std::collections::{HashMap, HashSet};

    let filter = space_id.map(|sid| MetadataFilter {
        space_id: Some(sid.to_string()),
        source_type: None,
        source_path: None,
        date_from: None,
        date_to: None,
        custom: None,
    });

    let chunks = match rag.list_documents(filter, 100_000).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("build_corpus_stats failed: {}", e);
            return Ok(crate::rag::CorpusStats {
                total_docs: 0,
                vocabulary: HashSet::new(),
                document_types: HashMap::new(),
                domain_terms: HashMap::new(),
                avg_doc_length: 0,
            });
        }
    };

    let mut seen_docs = HashSet::new();
    let mut document_types: HashMap<String, usize> = HashMap::new();
    let mut vocabulary = HashSet::new();
    let mut total_length: usize = 0;

    for chunk in &chunks {
        let doc_id = chunk.metadata.get("doc_id").cloned().unwrap_or_default();
        let is_new_doc = seen_docs.insert(doc_id);

        if is_new_doc {
            let ext = chunk
                .metadata
                .get("file_extension")
                .or_else(|| chunk.metadata.get("file_type"))
                .cloned()
                .unwrap_or_else(|| "unknown".to_string())
                .to_lowercase();
            *document_types.entry(ext).or_insert(0) += 1;
        }

        // Exclude space_metadata documents
        if chunk
            .metadata
            .get("doc_type")
            .map(|t| t == "space_metadata")
            .unwrap_or(false)
        {
            continue;
        }

        for word in chunk.snippet.split_whitespace() {
            let clean = word
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            if clean.len() > 2 {
                vocabulary.insert(clean);
            }
        }
        total_length += chunk.snippet.len();
    }

    let avg_doc_length = if chunks.is_empty() {
        0
    } else {
        total_length / chunks.len()
    };

    // Build domain term frequencies in a single pass over chunks.
    // Count how many chunks contain each term using per-chunk word sets.
    let total_docs_f = chunks.len().max(1) as f32;
    let mut term_doc_count: HashMap<String, usize> = HashMap::new();
    for chunk in &chunks {
        if chunk
            .metadata
            .get("doc_type")
            .map(|t| t == "space_metadata")
            .unwrap_or(false)
        {
            continue;
        }
        let chunk_words: HashSet<String> = chunk
            .snippet
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|w| w.len() > 2)
            .collect();
        for word in &chunk_words {
            *term_doc_count.entry(word.clone()).or_insert(0) += 1;
        }
    }
    let domain_terms: HashMap<String, f32> = term_doc_count
        .into_iter()
        .map(|(term, count)| (term, count as f32 / total_docs_f))
        .collect();

    tracing::debug!(
        "build_corpus_stats: {} chunks, {} unique docs, vocab={}, space_id={:?}",
        chunks.len(),
        seen_docs.len(),
        vocabulary.len(),
        space_id
    );

    Ok(crate::rag::CorpusStats {
        total_docs: seen_docs.len(),
        vocabulary,
        document_types,
        domain_terms,
        avg_doc_length,
    })
}

/// Estimate token count using chars/4 heuristic.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() + 3) / 4
}
