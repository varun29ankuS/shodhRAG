//! Context optimization for fast LLM responses
//!
//! Implements tiered context loading to minimize token processing time.
//! Based on industry best practices from OpenAI, Anthropic, and Google.

use chrono::Local;

/// Context tier determines how much system information to include
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextTier {
    /// Minimal: Just identity + time (~20 tokens, <1s processing)
    /// Use for: greetings, simple questions, confirmations
    Minimal,

    /// Standard: Basic capabilities (~100 tokens, ~2s processing)
    /// Use for: general questions, basic chat
    Standard,

    /// RAG: Document-aware context (~300 tokens, ~6s processing)
    /// Use for: document queries, code analysis
    RAG,

    /// SystemAware: Full system context (~1000 tokens, ~20s processing)
    /// Use for: "what am I working on?", system queries
    SystemAware,
}

/// Query intent classification for context routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextQueryIntent {
    /// Greeting: "hi", "hello", "thanks"
    Greeting,

    /// Simple question: "can you talk?", "how are you?"
    SimpleQuestion,

    /// Document query: RAG-based search
    DocumentQuery,

    /// Code analysis: explain code, find bugs
    CodeAnalysis,

    /// System query: "what files am I working on?"
    SystemQuery,
}

impl ContextQueryIntent {
    /// Classify a user query to determine appropriate context tier
    pub fn classify(query: &str) -> Self {
        let q = query.to_lowercase().trim().to_string();
        let word_count = q.split_whitespace().count();

        // Pattern 1: Greetings (very short, common phrases)
        if word_count <= 5 {
            let greetings = ["hi", "hello", "hey", "thanks", "thank you", "bye", "goodbye"];
            if greetings.iter().any(|g| q.contains(g)) {
                return ContextQueryIntent::Greeting;
            }
        }

        // Pattern 2: Simple questions (short, no complex intent)
        if word_count <= 10 {
            let simple_patterns = [
                "can you", "are you", "do you", "will you",
                "what is your", "who are you", "how are you"
            ];
            if simple_patterns.iter().any(|p| q.contains(p)) {
                return ContextQueryIntent::SimpleQuestion;
            }
        }

        // Pattern 3: System queries (asking about user's environment)
        let system_patterns = [
            "what am i working on", "what files", "what processes",
            "my system", "my computer", "my ram", "my cpu",
            "running processes", "active applications"
        ];
        if system_patterns.iter().any(|p| q.contains(p)) {
            return ContextQueryIntent::SystemQuery;
        }

        // Pattern 4: Code analysis (technical terms)
        let code_patterns = [
            "explain this", "what does this do", "how does",
            "function", "class", "method", "code", "bug",
            "error", "implement", "refactor"
        ];
        if code_patterns.iter().any(|p| q.contains(p)) {
            return ContextQueryIntent::CodeAnalysis;
        }

        // Pattern 5: Document queries (explicit or implicit)
        let doc_patterns = [
            "according to", "in the document", "what does",
            "tell me about", "explain", "summarize", "find"
        ];
        if doc_patterns.iter().any(|p| q.contains(p)) {
            return ContextQueryIntent::DocumentQuery;
        }

        // Default: treat as document query (safest for RAG system)
        ContextQueryIntent::DocumentQuery
    }

    /// Get the appropriate context tier for this intent
    pub fn context_tier(&self) -> ContextTier {
        match self {
            ContextQueryIntent::Greeting => ContextTier::Minimal,
            ContextQueryIntent::SimpleQuestion => ContextTier::Minimal,
            ContextQueryIntent::DocumentQuery => ContextTier::RAG,
            ContextQueryIntent::CodeAnalysis => ContextTier::Standard,
            ContextQueryIntent::SystemQuery => ContextTier::SystemAware,
        }
    }
}

/// Build context based on tier
pub fn build_tiered_context(tier: ContextTier) -> String {
    let mut context = String::new();
    let now = Local::now();

    // Tier 1: ALWAYS include (minimal overhead)
    context.push_str("You are Shodh - an intelligent AI assistant.\n");
    context.push_str("IMPORTANT: Always respond in the SAME language as the user's input (English for English, Hindi for Hindi, etc.).\n");
    context.push_str(&format!("Current time: {}\n\n", now.format("%Y-%m-%d %H:%M")));

    if tier == ContextTier::Minimal {
        return context;  // STOP HERE for greetings/simple questions
    }

    // Tier 2: Standard capabilities
    if matches!(tier, ContextTier::Standard | ContextTier::RAG | ContextTier::SystemAware) {
        context.push_str("# CAPABILITIES\n");
        context.push_str("- Answer questions about code and documents\n");
        context.push_str("- Provide clear, concise explanations\n");
        context.push_str("- Help with research and analysis\n\n");
    }

    // Tier 3: RAG-specific instructions
    if tier == ContextTier::RAG {
        context.push_str("# DOCUMENT SEARCH MODE\n");
        context.push_str("When answering from documents:\n");
        context.push_str("- ALWAYS cite sources with [1], [2], etc.\n");
        context.push_str("- Be precise and factual\n");
        context.push_str("- If information isn't in the provided context, say so\n\n");
    }

    // Tier 4: Full system awareness (ONLY when explicitly needed)
    if tier == ContextTier::SystemAware {
        // Import system functions directly
        use crate::system::os_integration::{get_system_info, list_running_processes};

        context.push_str("# SYSTEM INFORMATION\n");

        if let Ok(sys_info) = get_system_info() {
            context.push_str(&format!("OS: {} {}\n", sys_info.os, sys_info.os_version));
            context.push_str(&format!("Architecture: {}\n", sys_info.architecture));
            context.push_str(&format!("CPU Cores: {}\n", sys_info.cpu_count));
            context.push_str(&format!("Total Memory: {} MB\n", sys_info.total_memory_mb));
        }

        if let Ok(processes) = list_running_processes() {
            // Filter to interesting processes only
            let interesting: Vec<_> = processes.iter()
                .filter(|p| {
                    let name = p.name.to_lowercase();
                    name.contains("code") || name.contains("chrome") ||
                    name.contains("firefox") || name.contains("node") ||
                    name.contains("python") || name.contains("cargo")
                })
                .take(5)
                .collect();

            if !interesting.is_empty() {
                context.push_str("\nActive Applications:\n");
                for proc in interesting {
                    context.push_str(&format!("- {}\n", proc.name));
                }
            }
        }

        context.push_str("\n");
    }

    context
}

/// Auto-select context based on query analysis
pub fn build_context_for_query(query: &str) -> (String, ContextQueryIntent, ContextTier) {
    let intent = ContextQueryIntent::classify(query);
    let tier = intent.context_tier();
    let context = build_tiered_context(tier);

    (context, intent, tier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_greeting_classification() {
        assert_eq!(ContextQueryIntent::classify("hi"), ContextQueryIntent::Greeting);
        assert_eq!(ContextQueryIntent::classify("hello there"), ContextQueryIntent::Greeting);
        assert_eq!(ContextQueryIntent::classify("thanks!"), ContextQueryIntent::Greeting);
    }

    #[test]
    fn test_simple_question_classification() {
        assert_eq!(ContextQueryIntent::classify("can you talk?"), ContextQueryIntent::SimpleQuestion);
        assert_eq!(ContextQueryIntent::classify("are you working?"), ContextQueryIntent::SimpleQuestion);
        assert_eq!(ContextQueryIntent::classify("how are you?"), ContextQueryIntent::SimpleQuestion);
    }

    #[test]
    fn test_system_query_classification() {
        assert_eq!(ContextQueryIntent::classify("what am i working on?"), ContextQueryIntent::SystemQuery);
        assert_eq!(ContextQueryIntent::classify("what files are open?"), ContextQueryIntent::SystemQuery);
    }

    #[test]
    fn test_context_size() {
        let minimal = build_tiered_context(ContextTier::Minimal);
        let standard = build_tiered_context(ContextTier::Standard);
        let rag = build_tiered_context(ContextTier::RAG);

        // Minimal should be tiny
        assert!(minimal.len() < 200, "Minimal context too large: {} chars", minimal.len());

        // Standard should be moderate
        assert!(standard.len() < 500, "Standard context too large: {} chars", standard.len());

        // RAG should be larger but still reasonable
        assert!(rag.len() < 1000, "RAG context too large: {} chars", rag.len());

        // Should be increasing in size
        assert!(minimal.len() < standard.len());
        assert!(standard.len() < rag.len());
    }
}
