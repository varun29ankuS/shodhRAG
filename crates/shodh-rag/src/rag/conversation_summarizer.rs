//! Conversation Summarizer — compresses long conversation histories into
//! a rolling summary + recent messages to fit within context window budgets.
//!
//! Industry-standard approach: keep the last N messages verbatim for immediate
//! context, and summarize everything before into a compact representation
//! preserving key facts, entities, and decisions.

/// A compressed representation of conversation history.
pub struct CompressedHistory {
    /// Summary of older messages (None if conversation is short enough)
    pub summary: Option<String>,
    /// Recent messages kept verbatim
    pub recent_messages: Vec<(String, String)>, // (role, content)
}

/// Compress conversation history into a rolling summary + recent turns.
///
/// Strategy: keep the last `max_recent` messages verbatim, summarize everything
/// before that into topics + entities (rule-based, no LLM dependency).
pub fn compress_history(
    messages: &[(String, String)], // (role, content)
    max_recent: usize,
) -> CompressedHistory {
    if messages.len() <= max_recent {
        return CompressedHistory {
            summary: None,
            recent_messages: messages.to_vec(),
        };
    }

    let split_point = messages.len() - max_recent;
    let to_summarize = &messages[..split_point];
    let recent = &messages[split_point..];

    // Extract key facts from older messages (rule-based)
    let mut topics: Vec<String> = Vec::new();
    let mut entities: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();

    for (role, content) in to_summarize {
        // Track question topics from user messages
        if role == "user" || role == "User" {
            let topic: String = content.chars().take(80).collect();
            topics.push(topic.trim().to_string());
        }

        // Extract entities: capitalized words, file paths, numbers with units
        for word in content.split_whitespace() {
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '/' && c != '\\');
            if clean.is_empty() {
                continue;
            }

            // File paths
            if (clean.contains('/') || clean.contains('\\'))
                && clean.len() > 4
                && !files.contains(&clean.to_string())
            {
                files.push(clean.to_string());
                continue;
            }

            // File extensions
            if clean.contains('.') && clean.len() > 4 {
                let ext = clean.rsplit('.').next().unwrap_or("");
                if matches!(
                    ext,
                    "pdf" | "docx" | "xlsx" | "csv" | "txt" | "json" | "xml"
                ) && !files.contains(&clean.to_string())
                {
                    files.push(clean.to_string());
                    continue;
                }
            }

            // Proper nouns (capitalized, not start of sentence heuristic)
            if clean.len() > 2
                && clean.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                && !clean.chars().all(|c| c.is_uppercase()) // skip ALL_CAPS
                && !entities.contains(&clean.to_string())
            {
                entities.push(clean.to_string());
            }
        }
    }

    entities.sort();
    entities.dedup();
    entities.truncate(15);
    files.truncate(10);
    topics.truncate(5);

    let mut summary_parts = Vec::new();

    if !topics.is_empty() {
        summary_parts.push(format!(
            "Previous questions: {}",
            topics.join("; ")
        ));
    }
    if !entities.is_empty() {
        summary_parts.push(format!(
            "Key entities: {}",
            entities.join(", ")
        ));
    }
    if !files.is_empty() {
        summary_parts.push(format!(
            "Files discussed: {}",
            files.join(", ")
        ));
    }

    let summary = if summary_parts.is_empty() {
        None
    } else {
        Some(summary_parts.join(". ") + ".")
    };

    CompressedHistory {
        summary,
        recent_messages: recent.to_vec(),
    }
}

/// Format a CompressedHistory into a string suitable for LLM context.
pub fn format_compressed_history(history: &CompressedHistory) -> String {
    let mut result = String::new();

    if let Some(summary) = &history.summary {
        result.push_str("\nConversation History (for topic continuity ONLY — NOT a source of facts):\nSummary: ");
        result.push_str(summary);
        result.push_str("\n\nRecent Messages:\n");
    } else if !history.recent_messages.is_empty() {
        result.push_str("\nConversation History (for topic continuity ONLY — NOT a source of facts):\n");
    }

    for (role, content) in &history.recent_messages {
        result.push_str(&format!("{}: {}\n", role, content));
    }

    result
}
