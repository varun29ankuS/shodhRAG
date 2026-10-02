//! Query Rewriting Module
//!
//! Rewrites user queries using conversation context for better search results.
//! Uses LLM to expand queries with implicit context from conversation history.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewrittenQuery {
    pub original_query: String,
    pub rewritten_query: String,
    pub explanation: String,
    pub used_context: bool,
    pub should_retrieve: bool,    // Go/No-go decision
    pub retrieval_reason: String, // Why retrieve or not
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    pub topic: String,
    pub recent_messages: Vec<String>,
    pub concepts_mentioned: Vec<String>,
    pub files_discussed: Vec<String>,
    pub entities: Vec<String>,
}

impl Default for ConversationContext {
    fn default() -> Self {
        Self {
            topic: String::new(),
            recent_messages: Vec::new(),
            concepts_mentioned: Vec::new(),
            files_discussed: Vec::new(),
            entities: Vec::new(),
        }
    }
}

/// Query rewriter that uses conversation context to expand queries
pub struct QueryRewriter {
    /// Enable debug logging
    pub debug: bool,
}

impl QueryRewriter {
    pub fn new() -> Self {
        Self { debug: false }
    }

    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Decide if query needs document retrieval (go/no-go decision)
    ///
    /// Returns true if the query is asking about documents/information that needs lookup.
    /// Returns false for greetings, meta questions, or conversational queries.
    pub fn should_retrieve_documents(&self, query: &str) -> (bool, String) {
        let query_lower = query.to_lowercase();

        // No-retrieve: only exact or near-exact matches for very short queries
        let word_count = query.split_whitespace().count();

        // Single-word or two-word greetings/acknowledgments
        if word_count <= 2 {
            let exact_no_retrieve = [
                "hello",
                "hi",
                "hey",
                "thanks",
                "thank you",
                "bye",
                "goodbye",
                "ok",
                "okay",
                "yes",
                "no",
                "sure",
                "cool",
                "great",
            ];
            for pattern in &exact_no_retrieve {
                if query_lower == *pattern || query_lower.starts_with(&format!("{} ", pattern)) {
                    return (
                        false,
                        format!("Short greeting/acknowledgment — no documents needed"),
                    );
                }
            }
        }

        // Multi-word meta questions (only when the full query IS the meta question)
        if word_count <= 6 {
            let meta_patterns = [
                "what is your name",
                "who are you",
                "how are you",
                "what can you do",
                "what are you",
            ];
            for pattern in &meta_patterns {
                if query_lower.starts_with(pattern) {
                    return (
                        false,
                        format!("Meta question about assistant — no documents needed"),
                    );
                }
            }
        }

        // Go patterns (definitely need retrieval)
        let retrieve_patterns = [
            "find",
            "search",
            "show me",
            "get",
            "list",
            "what",
            "where",
            "when",
            "how",
            "why",
            "explain",
            "tell me about",
            "information",
            "details",
            "document",
            "file",
            "contract",
            "clause",
            "section",
            "provision",
            "requirement",
            "compliance",
            "audit",
            "liability",
            "payment",
            "terms",
        ];

        for pattern in &retrieve_patterns {
            if query_lower.contains(pattern) {
                return (
                    true,
                    format!(
                        "Query contains '{}' - likely needs document lookup",
                        pattern
                    ),
                );
            }
        }

        // Default: if query is > 3 words, assume it needs retrieval
        if query.split_whitespace().count() > 3 {
            return (true, "Multi-word query - likely informational".to_string());
        }

        // Very short queries without obvious patterns - skip retrieval
        (
            false,
            "Short query without clear retrieval intent".to_string(),
        )
    }

    /// Rewrite query using conversation context
    ///
    /// This is a core function that can be called with any LLM implementation.
    /// The caller provides the LLM generation function.
    pub async fn rewrite_with_context<F, Fut>(
        &self,
        query: &str,
        context: &ConversationContext,
        llm_generate: F,
    ) -> Result<RewrittenQuery>
    where
        F: FnOnce(String, usize) -> Fut,
        Fut: std::future::Future<Output = Result<String>>,
    {
        // Check if retrieval is needed first
        let (should_retrieve, retrieval_reason) = self.should_retrieve_documents(query);

        // If no meaningful context, return original
        if context.topic.is_empty()
            && context.recent_messages.is_empty()
            && context.concepts_mentioned.is_empty()
            && context.files_discussed.is_empty()
        {
            return Ok(RewrittenQuery {
                original_query: query.to_string(),
                rewritten_query: query.to_string(),
                explanation: "No relevant conversation context found".to_string(),
                used_context: false,
                should_retrieve,
                retrieval_reason,
            });
        }

        // Build context string
        let mut context_parts = Vec::new();

        if !context.topic.is_empty() {
            context_parts.push(format!("Topic: {}", context.topic));
        }

        if !context.recent_messages.is_empty() {
            context_parts.push(format!(
                "Recent conversation:\n{}",
                context.recent_messages.join("\n")
            ));
        }

        if !context.concepts_mentioned.is_empty() {
            context_parts.push(format!(
                "Concepts discussed: {}",
                context.concepts_mentioned.join(", ")
            ));
        }

        if !context.files_discussed.is_empty() {
            context_parts.push(format!(
                "Files discussed: {}",
                context.files_discussed.join(", ")
            ));
        }

        if !context.entities.is_empty() {
            context_parts.push(format!(
                "Entities mentioned: {}",
                context.entities.join(", ")
            ));
        }

        let context_str = context_parts.join("\n");

        // Build prompt for LLM
        let prompt = format!(
            r#"You are a query rewriting assistant. Given a user's query and conversation context, rewrite the query to be more specific and searchable.

Conversation Context:
{}

User's Query: "{}"

Rewrite the query to:
1. Replace pronouns (it, that, this) with specific terms from context
2. Add relevant concepts from conversation
3. Make the query more explicit and searchable
4. Keep it concise (max 2-3 sentences)

Rewritten Query:"#,
            context_str, query
        );

        // Call LLM
        match llm_generate(prompt, 150).await {
            Ok(rewritten) => {
                let rewritten = rewritten.trim().to_string();

                // Check if rewriting actually changed anything
                let changed = rewritten.to_lowercase() != query.to_lowercase();

                if self.debug {
                    tracing::debug!(original = %query, rewritten = %rewritten, changed = changed, "[QueryRewriter] Query rewrite result");
                }

                Ok(RewrittenQuery {
                    original_query: query.to_string(),
                    rewritten_query: rewritten.clone(),
                    explanation: if changed {
                        "Expanded query using conversation context".to_string()
                    } else {
                        "Query already specific enough".to_string()
                    },
                    used_context: changed,
                    should_retrieve,
                    retrieval_reason: retrieval_reason.clone(),
                })
            }
            Err(e) => {
                if self.debug {
                    tracing::warn!(error = %e, "[QueryRewriter] LLM failed");
                }

                // Fallback - return original query
                Ok(RewrittenQuery {
                    original_query: query.to_string(),
                    rewritten_query: query.to_string(),
                    explanation: format!("LLM rewriting failed: {}", e),
                    used_context: false,
                    should_retrieve,
                    retrieval_reason: retrieval_reason.clone(),
                })
            }
        }
    }

    /// Conversation-aware query reformulation.
    ///
    /// Resolves coreferences (pronouns, demonstratives, ellipsis) using
    /// conversation history so the search query is self-contained. Also
    /// generates expanded query variants for multi-query retrieval.
    ///
    /// Examples:
    /// - "who is anushree" → "who is anushree" (no change, already explicit)
    /// - "what is her salary" (after discussing anushree) → "what is anushree salary"
    /// - "tell me more" (after salary question) → "tell me more about anushree salary"
    /// - "and the PAN?" (after discussing anushree) → "what is anushree PAN"
    pub fn rewrite_rule_based(&self, query: &str, context: &ConversationContext) -> RewrittenQuery {
        let (should_retrieve, retrieval_reason) = self.should_retrieve_documents(query);

        let mut rewritten = query.to_string();
        let mut changes = Vec::new();
        let query_lower = query.trim().to_lowercase();

        // Detect bare search commands that need context expansion
        let bare_command_patterns = [
            "search online",
            "google",
            "search web",
            "look up online",
            "find online",
            "search internet",
            "web search",
            "search the web",
            "check online",
            "look online",
        ];

        let is_bare_command = bare_command_patterns
            .iter()
            .any(|p| query_lower == *p || query_lower.starts_with(&format!("{} ", p)));

        if is_bare_command {
            if let Some(last_query) = Self::find_last_user_query(&context.recent_messages) {
                rewritten = format!("{} {}", query.trim(), last_query.trim());
                changes.push(format!("Expanded bare command with previous query"));
            }
        }

        // --- Coreference Resolution ---
        // Find the primary entity being discussed (most recently mentioned person/thing)
        let primary_entity = Self::find_primary_entity(context);
        let last_topic = Self::find_last_topic(context);

        // 1. Resolve gendered pronouns: her/his/their → entity name
        let gendered_pronouns = [
            (" her ", " {entity} "),
            (" his ", " {entity} "),
            (" their ", " {entity} "),
            (" she ", " {entity} "),
            (" he ", " {entity} "),
            (" they ", " {entity} "),
            ("her ", "{entity} "),
            ("his ", "{entity} "),
        ];

        if let Some(ref entity) = primary_entity {
            for (pronoun, replacement) in &gendered_pronouns {
                let replacement = replacement.replace("{entity}", entity);
                if let Some(new) = Self::case_insensitive_replace(&rewritten, pronoun, &replacement)
                {
                    rewritten = new;
                    changes.push(format!("Resolved pronoun to '{}'", entity));
                    break; // Only replace once per query
                }
            }
        }

        // 2. Resolve "it"/"this"/"that" → file name if files discussed, else entity
        let demonstratives = [" it ", " this ", " that ", " it?", " this?", " that?"];
        let replacement_target = if !context.files_discussed.is_empty() {
            Some(context.files_discussed[0].clone())
        } else {
            primary_entity.clone()
        };

        if let Some(ref target) = replacement_target {
            for pronoun in &demonstratives {
                let replacement =
                    pronoun.replace(pronoun.trim_matches(|c: char| c == ' ' || c == '?'), target);
                if let Some(new) = Self::case_insensitive_replace(&rewritten, pronoun, &replacement)
                {
                    rewritten = new;
                    changes.push(format!("Resolved demonstrative to '{}'", target));
                    break;
                }
            }
        }

        // 3. Ellipsis resolution: very short follow-ups that reference previous topic
        //    "and the PAN?" → "what is {entity} PAN"
        //    "what about address?" → "what is {entity} address"
        //    "tell me more" → "tell me more about {entity/topic}"
        let word_count = query.split_whitespace().count();
        if word_count <= 5 && !changes.iter().any(|c| c.contains("Resolved")) {
            let ellipsis_patterns = [
                ("and the ", "what is {topic} "),
                ("and ", ""),
                ("what about ", "what is {topic} "),
                ("how about ", "what is {topic} "),
                ("tell me more", "tell me more about {topic}"),
                ("more about", "more about {topic}"),
                ("more details", "more details about {topic}"),
                ("elaborate", "elaborate on {topic}"),
                ("explain", "explain {topic}"),
            ];

            let topic_ref = primary_entity
                .as_deref()
                .or(last_topic.as_deref())
                .unwrap_or("");

            if !topic_ref.is_empty() {
                for (pattern, expansion) in &ellipsis_patterns {
                    if query_lower.starts_with(pattern) || query_lower == *pattern {
                        let expanded = expansion.replace("{topic}", topic_ref);
                        if !expanded.is_empty() {
                            let suffix = &query[pattern.len().min(query.len())..];
                            rewritten = format!("{}{}", expanded, suffix);
                        } else {
                            // "and X" → prepend entity context
                            let suffix = &query[pattern.len().min(query.len())..];
                            rewritten = format!("{} {}", topic_ref, suffix.trim());
                        }
                        changes.push(format!("Resolved ellipsis with topic '{}'", topic_ref));
                        break;
                    }
                }
            }
        }

        // 4. Short queries (1-2 words) with no resolution yet: append entity/topic context
        if word_count <= 2 && changes.is_empty() && !query_lower.is_empty() {
            if let Some(ref entity) = primary_entity {
                rewritten = format!("{} {}", entity, query.trim());
                changes.push(format!("Prepended entity '{}' to short query", entity));
            } else if !context.concepts_mentioned.is_empty() {
                let top_concepts: Vec<&str> = context
                    .concepts_mentioned
                    .iter()
                    .take(3)
                    .map(|c| c.as_str())
                    .collect();
                rewritten = format!("{} {}", query.trim(), top_concepts.join(" "));
                changes.push("Added relevant concepts to short query".to_string());
            }
        }

        let changed = rewritten != query;

        RewrittenQuery {
            original_query: query.to_string(),
            rewritten_query: rewritten,
            explanation: if changed {
                changes.join("; ")
            } else {
                "Query already self-contained".to_string()
            },
            used_context: changed,
            should_retrieve,
            retrieval_reason,
        }
    }

    /// Generate expanded query variants for multi-query retrieval.
    /// Returns the original query plus 1-2 variants that capture different
    /// phrasings or aspects of the same information need.
    pub fn expand_query(&self, query: &str, context: &ConversationContext) -> Vec<String> {
        let mut variants = vec![query.to_string()];
        let query_lower = query.to_lowercase();
        let word_count = query.split_whitespace().count();

        // Don't expand very short or very long queries
        if word_count < 2 || word_count > 20 {
            return variants;
        }

        // Strategy 1: Keyword extraction — strip question words and filler
        let keyword_query = Self::extract_keywords_for_search(&query_lower);
        if !keyword_query.is_empty() && keyword_query != query_lower {
            variants.push(keyword_query);
        }

        // Strategy 2: Synonym expansion for common document terms
        let synonym_query = Self::apply_synonyms(&query_lower);
        if let Some(syn) = synonym_query {
            variants.push(syn);
        }

        // Strategy 3: If context has entities, create entity-focused variant
        if !context.entities.is_empty() {
            let entity = &context.entities[0];
            let entity_lower = entity.to_lowercase();
            if !query_lower.contains(&entity_lower) {
                // Add entity-scoped variant: "salary details" → "anushree salary details"
                let entity_variant = format!("{} {}", entity, query.trim());
                variants.push(entity_variant);
            }
        }

        // Deduplicate
        let mut seen = std::collections::HashSet::new();
        variants.retain(|v| {
            let key = v.to_lowercase().trim().to_string();
            !key.is_empty() && seen.insert(key)
        });

        // Cap at 3 variants to avoid excessive search load
        variants.truncate(3);
        variants
    }

    // --- Internal helpers ---

    /// Find the most recently mentioned person/entity in conversation
    fn find_primary_entity(context: &ConversationContext) -> Option<String> {
        // Entities are already ordered by recency (extracted from recent messages)
        context.entities.first().cloned()
    }

    /// Find the last substantive topic from user messages
    fn find_last_topic(context: &ConversationContext) -> Option<String> {
        for msg in context.recent_messages.iter().rev() {
            // Only look at user messages
            if let Some(content) = msg
                .strip_prefix("user: ")
                .or_else(|| msg.strip_prefix("User: "))
            {
                let word_count = content.split_whitespace().count();
                if word_count >= 3 {
                    // Extract key content words (skip question words)
                    let keywords: Vec<&str> = content
                        .split_whitespace()
                        .filter(|w| {
                            let lower = w.to_lowercase();
                            !matches!(
                                lower.as_str(),
                                "what"
                                    | "is"
                                    | "are"
                                    | "the"
                                    | "a"
                                    | "an"
                                    | "of"
                                    | "in"
                                    | "for"
                                    | "to"
                                    | "and"
                                    | "or"
                                    | "can"
                                    | "you"
                                    | "me"
                                    | "tell"
                                    | "show"
                                    | "find"
                                    | "get"
                                    | "do"
                                    | "does"
                                    | "how"
                                    | "where"
                                    | "when"
                                    | "why"
                                    | "who"
                                    | "which"
                                    | "about"
                            )
                        })
                        .take(5)
                        .collect();

                    if !keywords.is_empty() {
                        return Some(keywords.join(" "));
                    }
                }
            }
        }
        None
    }

    /// Find the last substantive user query in conversation (for bare command expansion)
    fn find_last_user_query(messages: &[String]) -> Option<String> {
        let bare_commands = [
            "search online",
            "google",
            "search web",
            "look up online",
            "tell me more",
            "more details",
            "elaborate",
            "explain",
        ];

        for msg in messages.iter().rev() {
            if let Some(content) = msg
                .strip_prefix("user: ")
                .or_else(|| msg.strip_prefix("User: "))
            {
                let lower = content.trim().to_lowercase();
                let is_command = bare_commands.iter().any(|c| lower.starts_with(c));
                if !is_command && content.trim().len() > 3 {
                    return Some(content.trim().to_string());
                }
            }
        }
        None
    }

    /// Case-insensitive replacement that preserves surrounding context
    fn case_insensitive_replace(text: &str, pattern: &str, replacement: &str) -> Option<String> {
        let text_lower = text.to_lowercase();
        let pattern_lower = pattern.to_lowercase();

        if let Some(pos) = text_lower.find(&pattern_lower) {
            let mut result = String::with_capacity(text.len() + replacement.len());
            result.push_str(&text[..pos]);
            result.push_str(replacement);
            result.push_str(&text[pos + pattern.len()..]);
            Some(result)
        } else {
            None
        }
    }

    /// Strip question words and filler to produce a keyword-only search query.
    /// "what is the salary of anushree" → "salary anushree"
    fn extract_keywords_for_search(query: &str) -> String {
        let stop_words: std::collections::HashSet<&str> = [
            "what", "is", "are", "was", "were", "the", "a", "an", "of", "in", "for", "to", "and",
            "or", "can", "you", "me", "my", "tell", "show", "find", "get", "do", "does", "how",
            "where", "when", "why", "who", "which", "about", "please", "could", "would", "should",
            "there", "their", "from", "with", "that", "this", "have", "has", "had", "be", "been",
            "being", "it", "its", "i",
        ]
        .iter()
        .copied()
        .collect();

        let keywords: Vec<&str> = query
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|w| w.len() > 1 && !stop_words.contains(w))
            .collect();

        keywords.join(" ")
    }

    /// Apply domain-specific synonym expansion.
    /// Returns a variant query with one key term replaced by its synonym.
    fn apply_synonyms(query: &str) -> Option<String> {
        let synonym_pairs: &[(&str, &str)] = &[
            ("salary", "income compensation pay"),
            ("income", "salary earnings revenue"),
            ("address", "residence location"),
            ("phone", "mobile contact telephone"),
            ("email", "e-mail mail"),
            ("dob", "date of birth birthday"),
            ("date of birth", "dob birthday"),
            ("pan", "PAN permanent account number"),
            ("aadhar", "aadhaar UID unique identification"),
            ("aadhaar", "aadhar UID unique identification"),
            ("bank", "banking account financial"),
            ("balance", "amount total"),
            ("name", "full name"),
            ("spouse", "husband wife partner"),
            ("father", "parent dad"),
            ("mother", "parent mom"),
            ("children", "kids dependents"),
            ("employer", "company organization firm"),
            ("designation", "position title role job"),
            ("qualification", "education degree"),
            ("experience", "work history employment"),
        ];

        for (term, synonyms) in synonym_pairs {
            if query.contains(term) {
                // Pick the first synonym that isn't already in the query
                for syn in synonyms.split_whitespace() {
                    if !query.contains(syn) {
                        let variant = query.replacen(term, syn, 1);
                        return Some(variant);
                    }
                }
            }
        }
        None
    }
}

impl Default for QueryRewriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_pronoun_replacement() {
        let rewriter = QueryRewriter::new();
        let mut context = ConversationContext::default();
        context
            .files_discussed
            .push("query_rewriter.rs".to_string());

        let result = rewriter.rewrite_rule_based("How does it handle errors?", &context);

        assert!(result.used_context);
        assert!(result.rewritten_query.contains("query_rewriter.rs"));
    }

    #[test]
    fn test_empty_context() {
        let rewriter = QueryRewriter::new();
        let context = ConversationContext::default();

        let result = rewriter.rewrite_rule_based("What is vector search?", &context);

        assert!(!result.used_context);
        assert_eq!(result.original_query, result.rewritten_query);
    }

    #[test]
    fn test_gendered_pronoun_resolution() {
        let rewriter = QueryRewriter::new();
        let mut context = ConversationContext::default();
        context.entities.push("Anushree Sharma".to_string());

        let result = rewriter.rewrite_rule_based("what is her salary", &context);

        assert!(result.used_context);
        assert!(result.rewritten_query.contains("Anushree Sharma"));
        assert!(!result.rewritten_query.contains("her"));
    }

    #[test]
    fn test_ellipsis_resolution() {
        let rewriter = QueryRewriter::new();
        let mut context = ConversationContext::default();
        context.entities.push("Anushree".to_string());
        context
            .recent_messages
            .push("user: who is anushree".to_string());

        let result = rewriter.rewrite_rule_based("and the PAN?", &context);

        assert!(result.used_context);
        assert!(result.rewritten_query.to_lowercase().contains("anushree"));
        assert!(result.rewritten_query.to_lowercase().contains("pan"));
    }

    #[test]
    fn test_tell_me_more_resolution() {
        let rewriter = QueryRewriter::new();
        let mut context = ConversationContext::default();
        context.entities.push("Varun".to_string());
        context
            .recent_messages
            .push("user: what is varun's salary".to_string());

        let result = rewriter.rewrite_rule_based("tell me more", &context);

        assert!(result.used_context);
        assert!(result.rewritten_query.to_lowercase().contains("varun"));
    }

    #[test]
    fn test_query_expansion_synonyms() {
        let rewriter = QueryRewriter::new();
        let context = ConversationContext::default();

        let variants = rewriter.expand_query("what is the salary", &context);
        assert!(
            variants.len() >= 2,
            "Expected at least 2 variants, got {}",
            variants.len()
        );
        // Should have original + keyword variant or synonym variant
    }

    #[test]
    fn test_query_expansion_entity_context() {
        let rewriter = QueryRewriter::new();
        let mut context = ConversationContext::default();
        context.entities.push("Anushree".to_string());

        let variants = rewriter.expand_query("what is the salary", &context);
        // Should have a variant with "Anushree" prepended
        let has_entity_variant = variants.iter().any(|v| v.contains("Anushree"));
        assert!(
            has_entity_variant,
            "Expected entity variant in {:?}",
            variants
        );
    }

    #[test]
    fn test_keyword_extraction() {
        let keywords = QueryRewriter::extract_keywords_for_search("what is the salary of anushree");
        assert!(keywords.contains("salary"));
        assert!(keywords.contains("anushree"));
        assert!(!keywords.contains("what"));
        assert!(!keywords.contains("the"));
    }
}
