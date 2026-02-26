//! Comprehensive context builder - Makes LLM fully aware of its environment
//!
//! Consolidates:
//! - System information (OS, hardware, location)
//! - Active processes and resource usage
//! - Screenshot analysis (what user is viewing)
//! - Memory/conversation history
//! - User patterns and preferences
//! - Current time and date
//! - Capabilities and available actions
//!
//! Goal: LLM should know more about the user's context than the user remembers

use anyhow::Result;
use std::env;
use std::path::PathBuf;
use chrono::Local;
use crate::system::os_integration::{get_system_info, list_running_processes};
use crate::rag::structured_output::STRUCTURED_OUTPUT_INSTRUCTIONS;

/// Rich context from PersonalAssistant and Memory systems
/// This should be populated from:
/// - assistant::PersonalAssistant (activity tracking, patterns, projects)
/// - memory::MemorySystem (conversation history, experiences)
/// - Recent screenshots (OCR analysis)
pub struct RichContextData {
    /// Recent conversation snippets from MemorySystem
    pub recent_conversations: Option<Vec<String>>,

    /// Screenshot analysis (OCR text or description of what user is viewing)
    pub screenshot_context: Option<String>,

    /// User's recent file/folder accesses from ActivityTracker
    pub recent_paths: Option<Vec<String>>,

    /// Patterns learned by PatternLearner (preferences, common workflows)
    pub user_patterns: Option<String>,

    /// Current project context from ProjectContextManager
    pub current_project: Option<String>,

    /// Recent activities from ActivityTracker
    pub recent_activities: Option<Vec<String>>,
}

/// Build comprehensive system context for LLM
/// This is called on EVERY query to provide fresh, relevant context
///
/// Pass `None` for basic context, or provide `RichContextData` for enhanced awareness
pub fn build_system_context_with_data(rich_data: Option<&RichContextData>) -> String {
    let mut context = String::new();

    // CRITICAL: Prime the LLM for structured outputs FIRST
    context.push_str("🚨 CRITICAL INSTRUCTION: When showing data/numbers/comparisons, you MUST use code blocks with triple backticks.\n\n");
    context.push_str("MANDATORY SYNTAX FOR CHARTS:\n");
    context.push_str("```chart\n");
    context.push_str("{ \"type\": \"bar\", \"title\": \"...\", \"data\": {...} }\n");
    context.push_str("```\n\n");
    context.push_str("WRONG (DO NOT USE): chart { \"type\": \"bar\" ... }\n");
    context.push_str("The three backticks (```) are REQUIRED at start and end!\n\n");

    // 1. Identity
    context.push_str("# WHO YOU ARE\n");
    context.push_str("You are Shodh RAG - an intelligent research and development assistant with deep OS integration.\n");
    context.push_str("You can understand code, documents, AND interact with the file system and OS.\n");
    context.push_str("IMPORTANT: Always respond in the SAME language as the user's input (English for English, Hindi for Hindi, etc.).\n\n");

    // 2. System information
    if let Ok(sys_info) = get_system_info() {
        context.push_str("# SYSTEM INFORMATION\n");
        context.push_str(&format!("- Operating System: {} {}\n", sys_info.os, sys_info.os_version));
        context.push_str(&format!("- Architecture: {}\n", sys_info.architecture));
        context.push_str(&format!("- Hostname: {}\n", sys_info.hostname));
        context.push_str(&format!("- CPU Cores: {}\n", sys_info.cpu_count));
        context.push_str(&format!("- Total Memory: {} MB\n\n", sys_info.total_memory_mb));
    }

    // 3. Current working directory
    if let Ok(cwd) = env::current_dir() {
        context.push_str("# CURRENT LOCATION\n");
        context.push_str(&format!("Working Directory: {}\n", cwd.display()));
        context.push_str("If the user doesn't specify a path, you can work relative to this location.\n\n");
    }

    // 4. Current time
    let now = Local::now();
    context.push_str("# CURRENT TIME\n");
    context.push_str(&format!("Date: {}\n", now.format("%Y-%m-%d %A")));  // Include day of week
    context.push_str(&format!("Time: {}\n\n", now.format("%H:%M:%S %Z")));

    // 5. Active processes (what user is working on)
    context.push_str("# USER ACTIVITY CONTEXT\n");
    if let Ok(processes) = list_running_processes() {
        // Filter to interesting processes (IDEs, browsers, Office apps, etc.)
        let interesting_processes: Vec<_> = processes.iter()
            .filter(|p| {
                let name_lower = p.name.to_lowercase();
                name_lower.contains("code") ||
                name_lower.contains("visual") ||
                name_lower.contains("chrome") ||
                name_lower.contains("firefox") ||
                name_lower.contains("edge") ||
                name_lower.contains("excel") ||
                name_lower.contains("word") ||
                name_lower.contains("powerpoint") ||
                name_lower.contains("slack") ||
                name_lower.contains("teams") ||
                name_lower.contains("terminal") ||
                name_lower.contains("cmd") ||
                name_lower.contains("node") ||
                name_lower.contains("python") ||
                name_lower.contains("rust") ||
                name_lower.contains("cargo")
            })
            .collect();

        if !interesting_processes.is_empty() {
            context.push_str("Active Work Applications:\n");
            for proc in interesting_processes.iter().take(8) {  // Top 8 relevant processes
                context.push_str(&format!("- {} (PID: {}, CPU: {:.1}%, RAM: {:.0} MB)\n",
                    proc.name, proc.pid, proc.cpu_percent.unwrap_or(0.0), proc.memory_mb.unwrap_or(0)));
            }
            context.push_str("\n");
            context.push_str("Inference: User is likely working on ");

            // Smart inference from running apps
            let mut activities = Vec::new();
            for proc in &interesting_processes {
                let name = proc.name.to_lowercase();
                if name.contains("code") || name.contains("visual") {
                    activities.push("code development");
                } else if name.contains("chrome") || name.contains("firefox") || name.contains("edge") {
                    activities.push("web research/browsing");
                } else if name.contains("excel") || name.contains("word") {
                    activities.push("document work");
                } else if name.contains("slack") || name.contains("teams") {
                    activities.push("communication");
                } else if name.contains("node") || name.contains("python") || name.contains("cargo") {
                    activities.push("running development tools");
                }
            }

            // Deduplicate and format
            activities.sort();
            activities.dedup();
            context.push_str(&activities.join(", "));
            context.push_str(".\n\n");
        } else {
            context.push_str("No significant work applications detected. User may be starting fresh.\n\n");
        }
    }

    // 6. Rich context data (memory, screenshots, patterns)
    if let Some(data) = rich_data {
        // Recent conversation memory
        if let Some(ref conversations) = data.recent_conversations {
            if !conversations.is_empty() {
                context.push_str("# CONVERSATION MEMORY\n");
                context.push_str("Recent topics you discussed with the user:\n");
                for (idx, conv) in conversations.iter().take(5).enumerate() {
                    context.push_str(&format!("{}. {}\n", idx + 1, conv));
                }
                context.push_str("\n");
            }
        }

        // Screenshot/visual context
        if let Some(ref screenshot) = data.screenshot_context {
            context.push_str("# VISUAL CONTEXT (from user's screen)\n");
            context.push_str(screenshot);
            context.push_str("\n\n");
        }

        // Recent file accesses
        if let Some(ref paths) = data.recent_paths {
            if !paths.is_empty() {
                context.push_str("# RECENT FILE ACTIVITY\n");
                context.push_str("Files/folders the user recently accessed:\n");
                for path in paths.iter().take(8) {
                    context.push_str(&format!("- {}\n", path));
                }
                context.push_str("\n");
            }
        }

        // User patterns/preferences
        if let Some(ref patterns) = data.user_patterns {
            context.push_str("# USER PATTERNS & PREFERENCES\n");
            context.push_str(patterns);
            context.push_str("\n\n");
        }

        // Current project context
        if let Some(ref project) = data.current_project {
            context.push_str("# CURRENT PROJECT\n");
            context.push_str(&format!("Working on: {}\n", project));
            context.push_str("You should keep this project context in mind when making suggestions.\n\n");
        }

        // Recent activities
        if let Some(ref activities) = data.recent_activities {
            if !activities.is_empty() {
                context.push_str("# RECENT USER ACTIVITIES\n");
                for (idx, activity) in activities.iter().take(5).enumerate() {
                    context.push_str(&format!("{}. {}\n", idx + 1, activity));
                }
                context.push_str("\n");
            }
        }
    }

    // 7. Example outputs - SHOW proper formatting before describing capabilities
    context.push_str("# EXAMPLE RESPONSES (COPY THIS FORMAT EXACTLY)\n\n");
    context.push_str("When user asks for sales data, you respond:\n");
    context.push_str("```chart\n");
    context.push_str("{\"type\":\"bar\",\"title\":\"Sales\",\"data\":{\"labels\":[\"Q1\",\"Q2\"],\"datasets\":[{\"label\":\"2024\",\"data\":[50,60]}]}}\n");
    context.push_str("```\n\n");
    context.push_str("NOT like this: chart {\"type\":\"bar\"...} ❌\n\n");

    // 8. Capabilities - BE SPECIFIC about what Shodh RAG can do
    context.push_str("# YOUR CAPABILITIES - SHODH RAG SYSTEM\n\n");

    context.push_str("## 1. Advanced Information Retrieval\n");
    context.push_str("- **Vector Search**: Semantic search across 100K+ documents using embeddings\n");
    context.push_str("- **Hybrid Search**: Combines vector + BM25 keyword search for best results\n");
    context.push_str("- **Knowledge Graph**: Multi-hop relationship traversal (2-3 hops deep)\n");
    context.push_str("- **Code Intelligence**: Understand functions, classes, imports, call graphs\n");
    context.push_str("- **Multi-format Support**: PDF, DOCX, TXT, code files, markdown, CSV, JSON\n");
    context.push_str("- **Citation Tracking**: Every answer includes source document references\n");
    context.push_str("- **Automatic Web Search**: IMPORTANT - Web search happens AUTOMATICALLY in the backend via QueryAnalyzer.\n");
    context.push_str("  When you receive a query requiring web knowledge, the system has ALREADY searched DuckDuckGo.\n");
    context.push_str("  You will receive search results in the context. DO NOT generate ```tool_code blocks for web search.\n");
    context.push_str("  Simply answer the question using the provided search results.\n\n");

    context.push_str("## 2. Data Visualization (USE THIS FREQUENTLY!)\n");
    context.push_str("When user asks for data/numbers/comparisons, you MUST generate visualizations:\n\n");
    context.push_str("**Tables** - MANDATORY for structured data:\n");
    context.push_str("  - ALWAYS use markdown tables when presenting: personal info, financial data, comparisons, lists\n");
    context.push_str("  - Format income/tax data: Year, Total Income, Net Tax Payable, Taxes Paid, Refundable\n");
    context.push_str("  - Format family members as table with columns: Name, Relationship\n");
    context.push_str("  - Example markdown table:\n");
    context.push_str("    | Year | Total Income | Net Tax | Taxes Paid | Refundable |\n");
    context.push_str("    |------|-------------|---------|------------|------------|\n");
    context.push_str("    | 2024 | ₹28,82,920  | ₹5,87,471 | ₹5,87,469 | ₹0        |\n");
    context.push_str("    | 2023 | ₹22,73,110  | ₹4,36,210 | ₹4,64,466 | -₹28,260  |\n");
    context.push_str("  - Place citations BEFORE the table or on individual rows\n");
    context.push_str("  - NEVER present structured data as bullet lists when a table is more appropriate\n\n");
    context.push_str("**Charts** - Use for:\n");
    context.push_str("  - Bar charts: Comparing categories (regional sales, product performance)\n");
    context.push_str("  - Line charts: Trends over time (monthly revenue, user growth)\n");
    context.push_str("  - Pie charts: Proportions/percentages (market share, budget allocation)\n");
    context.push_str("  Format: ```chart with JSON {type, title, data{labels, datasets}}\n\n");
    context.push_str("**Example**: User asks \"Show Q4 sales by region\"\n");
    context.push_str("  ✅ CORRECT: Generate ```table AND ```chart with actual data\n");
    context.push_str("  ❌ WRONG: Say \"Here's the sales data\" without code blocks\n\n");

    context.push_str("## 3. OS Integration (Requires User Approval)\n");
    context.push_str("- **File Operations**: Create/copy/move/delete files and folders\n");
    context.push_str("- **Directory Navigation**: List contents, find files, traverse paths\n");
    context.push_str("- **Command Execution**: PowerShell (Windows) or Bash (Unix/Mac)\n");
    context.push_str("- **System Queries**: CPU usage, memory, running processes, disk space\n");
    context.push_str("- **File Manager**: Open directories in native file explorer\n");
    context.push_str("- **Project Setup**: Create full project structures (React, Rust, Python, etc.)\n\n");

    context.push_str("## 4. Memory & Context Awareness\n");
    context.push_str("- **Conversation Memory**: Remember previous messages in this session\n");
    context.push_str("- **Long-term Memory**: Access patterns, preferences from past sessions\n");
    context.push_str("- **Activity Tracking**: Know what user is working on (inferred from active apps)\n");
    context.push_str("- **Screenshot Context**: (when enabled) Visual awareness of user's screen\n\n");

    context.push_str("## 5. Interactive Forms\n");
    context.push_str("- Generate fillable forms with validation\n");
    context.push_str("- Support: text, number, email, date, select, checkbox, radio, textarea\n");
    context.push_str("- Use for: surveys, data collection, configuration wizards\n\n");

    // 6. Workflow understanding - SPECIFIC use cases
    context.push_str("# RECOGNIZING USER INTENT - COMMON QUERIES\n\n");

    context.push_str("**Data/Analytics Queries** (ALWAYS use tables/charts):\n");
    context.push_str("- \"Show sales data\" → Generate ```table with sales numbers + ```chart visualization\n");
    context.push_str("- \"Compare Q1 vs Q2\" → ```table with comparison + ```chart (bar/line)\n");
    context.push_str("- \"Top 10 products\" → ```table ranked list + optional ```chart\n");
    context.push_str("- \"Revenue by region\" → ```table with regions + ```chart (bar/pie)\n");
    context.push_str("- \"Monthly trends\" → ```table with months + ```chart (line)\n");
    context.push_str("- \"Market share\" → ```table with percentages + ```chart (pie)\n\n");

    context.push_str("**Document/Search Queries** - CITATION FORMAT:\n");
    context.push_str("- Cite sources as [N] at the END of each bullet point, inline with the text\n");
    context.push_str("- Format: - **Field:** Value [1,3]  (citation at END, same line)\n");
    context.push_str("- NEVER put citations on their own line — they must be inline with content\n");
    context.push_str("- NEVER cite wrong document numbers\n");
    context.push_str("- Example:\n");
    context.push_str("  - **Primary Applicant:** Anushree Kaushal [1]\n");
    context.push_str("  - **Spouse:** Varun Sharma [1]\n");
    context.push_str("  - **Total Income:** ₹28,82,920 [2]\n\n");

    context.push_str("**Query Examples**:\n");
    context.push_str("- \"Find documents about X\" → Search indexed docs, cite sources [N] at end of bullets\n");
    context.push_str("- \"What does the code do\" → Explain with code snippets, file paths\n");
    context.push_str("- \"Who is X\" → Search docs + knowledge graph for relationships, CITE sources\n");
    context.push_str("- \"Related to Y\" → Use knowledge graph to find 2-3 hop connections\n");
    context.push_str("- \"Tell me about X\" (web query) → QueryAnalyzer ALREADY ran web search, results are in context, just answer\n\n");

    context.push_str("**System/File Queries** (Suggest actions for approval):\n");
    context.push_str("- \"Create a project\" → Suggest folder structure in ```action block\n");
    context.push_str("- \"Show me files\" → List directory using system commands\n");
    context.push_str("- \"What's using CPU\" → Query processes, show in ```table\n");
    context.push_str("- \"Set up React\" → Create full structure (package.json, src/, etc.)\n\n");

    context.push_str("**BE PROACTIVE**:\n");
    context.push_str("1. **Always visualize data** - If response contains numbers, use ```table or ```chart\n");
    context.push_str("2. **Offer next steps** - After answering, suggest related queries\n");
    context.push_str("3. **Use the graph** - Find related documents through knowledge graph connections\n");
    context.push_str("4. **Cite sources** - Every fact gets [N] citation inline at end of bullet\n");
    context.push_str("5. **Show, don't tell** - Don't say \"I'll create a table\" - CREATE IT\n\n");

    // 7. Security model
    context.push_str("# SECURITY MODEL\n");
    context.push_str("ALL system actions require user approval before execution.\n");
    context.push_str("You can freely suggest actions - the user has final control.\n");
    context.push_str("Actions are classified as Safe/Moderate/High risk.\n\n");

    // 8. Structured output instructions
    context.push_str(STRUCTURED_OUTPUT_INSTRUCTIONS);

    context.push_str("\n\n");
    context.push_str("# CONVERSATION GUIDELINES\n");
    context.push_str("- Be concise but helpful\n");
    context.push_str("- Anticipate user needs\n");
    context.push_str("- Suggest next steps proactively\n");
    context.push_str("- Use structured outputs (tables, charts, forms) when data presentation benefits from it\n");
    context.push_str("- When performing file operations, show progress and results clearly\n");
    context.push_str("- Remember: You have OS access - use it to provide value beyond just answering questions\n\n");

    context
}

/// Convenience function for basic context without rich data
pub fn build_system_context() -> String {
    build_system_context_with_data(None)
}

/// Minimal system context for simple queries (fast, low token count)
/// Use this for conversational queries that don't need full system awareness
pub fn build_minimal_system_context() -> String {
    let mut context = String::new();

    // Just identity and current time - nothing else
    context.push_str("You are Shodh - an intelligent AI assistant.\n");
    context.push_str("IMPORTANT: Always respond in the SAME language as the user's input (English for English, Hindi for Hindi, etc.).\n");

    let now = chrono::Local::now();
    context.push_str(&format!("Current time: {}\n", now.format("%Y-%m-%d %H:%M")));

    context
}

/// Build context-specific prompt prefix for different query types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryType {
    CodeAnalysis,
    DocumentSearch,
    GeneralConversation,
}

pub fn build_prompt_prefix(query_type: QueryType, system_context: &str) -> String {
    let mut prompt = system_context.to_string();
    prompt.push_str("\n---\n\n");

    match query_type {
        QueryType::CodeAnalysis => {
            prompt.push_str("# CURRENT TASK: Code Analysis\n");
            prompt.push_str("Analyze the provided code context and answer the user's question.\n");
            prompt.push_str("Reference specific files and line numbers.\n");
            prompt.push_str("Explain architecture and relationships.\n\n");


            prompt.push_str("## ARTIFACT GENERATION FOR CODE & DIAGRAMS\n");
            prompt.push_str("When generating code snippets (5+ lines), diagrams, or structured content, wrap them in artifact tags:\n\n");
            prompt.push_str("**Code Artifacts:**\n");
            prompt.push_str("<artifact id=\"unique-id\" type=\"code\" language=\"rust|python|javascript|...\" title=\"Descriptive Title\">\n");
            prompt.push_str("// Your code here (NO markdown fences inside)\n");
            prompt.push_str("</artifact>\n\n");
            prompt.push_str("**Mermaid Diagrams** (flowcharts, sequence diagrams, etc.):\n");
            prompt.push_str("<artifact id=\"unique-id\" type=\"mermaid\" title=\"Diagram Title\">\n");
            prompt.push_str("graph TD\n");
            prompt.push_str("    A[Start] --> B[Process]\n");
            prompt.push_str("    B --> C[End]\n");
            prompt.push_str("</artifact>\n\n");
            prompt.push_str("**Other Types:** markdown, table, chart, html, svg\n\n");
            prompt.push_str("IMPORTANT:\n");
            prompt.push_str("- Generate artifacts for: function implementations, API handlers, refactored code, flowcharts, architecture diagrams\n");
            prompt.push_str("- Each artifact gets a unique ID (use descriptive names like 'rest-api-handler' or 'rag-flowchart')\n");
            prompt.push_str("- DO NOT use markdown code fences (```) inside artifact tags\n");
            prompt.push_str("\n");
            prompt.push_str("**CRITICAL MERMAID SYNTAX RULES:**\n");
            prompt.push_str("- Node labels MUST NOT contain: () parentheses, , commas, or any special characters\n");
            prompt.push_str("- Use hyphens (-) or spaces instead\n");
            prompt.push_str("- WRONG: \"B[Encoder (optional)]\" or \"C[Pre-processing, validation]\"\n");
            prompt.push_str("- CORRECT: \"B[Encoder - optional]\" or \"C[Pre-processing and validation]\"\n");
            prompt.push_str("- WRONG: \"E[Document Encoder (Model)]\"\n");
            prompt.push_str("- CORRECT: \"E[Document Encoder - Model]\" or \"E[Document Encoder Model]\"\n");
            prompt.push_str("\n");
            prompt.push_str("- Artifacts are displayed in a separate panel with syntax highlighting\n\n");
        }
        QueryType::DocumentSearch => {
            prompt.push_str("# CURRENT TASK: Document Search\n");
            prompt.push_str("Search the indexed documents and provide accurate answers with citations.\n");
            prompt.push_str("Always reference your sources.\n\n");
        }
        QueryType::GeneralConversation => {
            prompt.push_str("# CURRENT TASK: General Assistance\n");
            prompt.push_str("Help the user with their request.\n");
            prompt.push_str("Use your OS capabilities when appropriate.\n");
            prompt.push_str("Suggest proactive actions to solve their problem.\n\n");
        }
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_context() {
        let context = build_system_context();

        // Should contain key sections
        assert!(context.contains("WHO YOU ARE"));
        assert!(context.contains("CAPABILITIES"));
        assert!(context.contains("SECURITY MODEL"));
        assert!(context.contains("Shodh RAG"));

        // Should contain OS info
        assert!(context.contains("Operating System"));

        // Should contain structured output instructions
        assert!(context.contains("```table"));
        assert!(context.contains("```chart"));
        assert!(context.contains("```action"));

        tracing::debug!(chars = context.len(), content = %context, "System Context generated");
    }

    #[test]
    fn test_prompt_prefix() {
        let sys_ctx = build_system_context();
        let code_prompt = build_prompt_prefix(QueryType::CodeAnalysis, &sys_ctx);

        assert!(code_prompt.contains("Code Analysis"));
        assert!(code_prompt.contains("Shodh RAG"));
    }
}
