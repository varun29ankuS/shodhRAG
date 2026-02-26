//! Retrieval Decision System
//!
//! Sophisticated go/no-go decision making for document retrieval.
//! Analyzes query intent, domain relevance, and selects optimal retrieval strategy.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static ARITHMETIC_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\d\s*[+\-*/]\s*\d").expect("arithmetic regex is valid")
});
static YEAR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b(19|20)\d{2}\b").expect("year regex is valid")
});

// ============================================================================
// Core Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum QueryIntent {
    // No retrieval needed
    Greeting,
    MetaQuestion,
    Clarification,
    SimpleAcknowledgment,
    FollowUpRequest,  // "show me that in a chart", "do the same for X"

    // Simple retrieval
    FactualLookup,
    DocumentSearch,
    DefinitionQuery,

    // Complex retrieval
    ComparativeAnalysis,
    AggregationQuery,
    FilteredSearch,
    MultiHopReasoning,
    TemporalQuery,

    // May not need retrieval
    Calculation,
    GeneralKnowledge,

    // No retrieval needed - generative tasks
    CreativeGeneration,
    ExampleCreation,

    // Web search needed
    CurrentEvents,
    RealTimeInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RetrievalStrategy {
    /// Simple top-K retrieval from local documents
    TopK { k: usize },

    /// Retrieve many, then filter
    FilteredSearch {
        initial_k: usize,
        filters: Vec<String>, // Filter descriptions
    },

    /// Multiple search stages
    MultiStage {
        stages: Vec<SearchStage>,
    },

    /// Web search only (current events, general knowledge not in corpus)
    WebSearch {
        query: String,
        max_results: usize,
    },

    /// Hybrid: Local documents + Web search
    HybridSearch {
        local_k: usize,
        web_results: usize,
    },

    /// No retrieval needed
    NoRetrieval {
        reason: String,
        llm_can_answer: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchStage {
    pub stage_name: String,
    pub search_query: String,
    pub max_results: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevanceScore {
    pub corpus_coverage: f32,    // 0.0 - 1.0: how many query terms exist in corpus
    pub domain_match: f32,        // 0.0 - 1.0: how well query matches document domains
    pub term_frequency: f32,      // 0.0 - 1.0: are query terms common or rare
    pub overall_confidence: f32,  // Combined score
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequirements {
    pub needs_filtering: bool,
    pub date_range: Option<DateRange>,
    pub numeric_conditions: Vec<NumericCondition>,
    pub entity_references: Vec<String>,
    pub document_type_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateRange {
    pub field: String,
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumericCondition {
    pub field: String,
    pub operator: ComparisonOp,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComparisonOp {
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Equal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalDecision {
    pub should_retrieve: bool,
    pub strategy: RetrievalStrategy,
    pub estimated_docs_needed: usize,
    pub confidence: f32,
    pub reasoning: String,
    pub fallback_plan: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryAnalysis {
    pub intent: QueryIntent,
    pub relevance: RelevanceScore,
    pub requirements: QueryRequirements,
    pub decision: RetrievalDecision,
}

#[derive(Debug, Clone)]
pub struct CorpusStats {
    pub total_docs: usize,
    pub vocabulary: HashSet<String>,
    pub document_types: HashMap<String, usize>,
    pub domain_terms: HashMap<String, f32>,
    pub avg_doc_length: usize,
}

impl Default for CorpusStats {
    fn default() -> Self {
        Self {
            total_docs: 0,
            vocabulary: HashSet::new(),
            document_types: HashMap::new(),
            domain_terms: HashMap::new(),
            avg_doc_length: 0,
        }
    }
}

// ============================================================================
// Query Analyzer - Main Entry Point
// ============================================================================

pub struct QueryAnalyzer {
    intent_classifier: IntentClassifier,
    domain_matcher: DomainMatcher,
    requirement_extractor: RequirementExtractor,
    strategy_selector: StrategySelector,
}

impl QueryAnalyzer {
    pub fn new() -> Self {
        Self {
            intent_classifier: IntentClassifier::new(),
            domain_matcher: DomainMatcher::new(),
            requirement_extractor: RequirementExtractor::new(),
            strategy_selector: StrategySelector::new(),
        }
    }

    pub fn analyze(&self, query: &str, corpus_stats: &CorpusStats) -> QueryAnalysis {
        // Step 1: Classify intent
        let intent = self.intent_classifier.classify(query);

        // Step 2: Check domain relevance
        let relevance = self.domain_matcher.check_relevance(query, corpus_stats);

        // Step 3: Extract requirements (filters, date ranges, etc.)
        let requirements = self.requirement_extractor.extract(query);

        // Step 4: Decide retrieval strategy
        let decision = self.strategy_selector.decide(
            query,
            &intent,
            &relevance,
            &requirements,
            corpus_stats,
        );

        QueryAnalysis {
            intent,
            relevance,
            requirements,
            decision,
        }
    }
}

impl Default for QueryAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Intent Classifier
// ============================================================================

pub struct IntentClassifier {}

impl IntentClassifier {
    pub fn new() -> Self {
        Self {}
    }

    pub fn classify(&self, query: &str) -> QueryIntent {
        let query_lower = query.to_lowercase();
        let word_count = query.split_whitespace().count();

        // Greetings (highest priority)
        if self.is_greeting(&query_lower, word_count) {
            return QueryIntent::Greeting;
        }

        // Simple acknowledgments
        if self.is_acknowledgment(&query_lower, word_count) {
            return QueryIntent::SimpleAcknowledgment;
        }

        // Meta questions
        if self.is_meta_question(&query_lower) {
            return QueryIntent::MetaQuestion;
        }

        // Clarification
        if self.is_clarification(&query_lower, word_count) {
            return QueryIntent::Clarification;
        }

        // Follow-up requests (check before document search to avoid false positives)
        if self.is_follow_up(&query_lower) {
            return QueryIntent::FollowUpRequest;
        }

        // Creative/generative queries (check before calculations)
        if self.is_creative_generation(&query_lower) {
            return QueryIntent::CreativeGeneration;
        }

        if self.is_example_creation(&query_lower) {
            return QueryIntent::ExampleCreation;
        }

        // Calculations
        if self.is_calculation(&query_lower) {
            return QueryIntent::Calculation;
        }

        // Current events / real-time info (needs web search)
        if self.is_current_events(&query_lower) {
            return QueryIntent::CurrentEvents;
        }

        if self.is_realtime_info(&query_lower) {
            return QueryIntent::RealTimeInfo;
        }

        // Filtered search (has conditions)
        if self.is_filtered_search(&query_lower) {
            return QueryIntent::FilteredSearch;
        }

        // Comparative analysis
        if self.is_comparative(&query_lower) {
            return QueryIntent::ComparativeAnalysis;
        }

        // Aggregation
        if self.is_aggregation(&query_lower) {
            return QueryIntent::AggregationQuery;
        }

        // Temporal query
        if self.is_temporal(&query_lower) {
            return QueryIntent::TemporalQuery;
        }

        // Multi-hop reasoning
        if self.is_multi_hop(&query_lower) {
            return QueryIntent::MultiHopReasoning;
        }

        // Document search
        if self.is_document_search(&query_lower) {
            return QueryIntent::DocumentSearch;
        }

        // Definition query
        if self.is_definition(&query_lower) {
            return if self.is_document_specific(&query_lower) {
                QueryIntent::DefinitionQuery
            } else {
                QueryIntent::GeneralKnowledge
            };
        }

        // Default: factual lookup if multi-word
        if word_count > 3 {
            QueryIntent::FactualLookup
        } else {
            QueryIntent::GeneralKnowledge
        }
    }

    fn is_greeting(&self, query: &str, word_count: usize) -> bool {
        if word_count > 5 {
            return false;
        }
        let greetings = [
            "hello", "hi", "hey", "greetings", "good morning", "good afternoon",
            "good evening", "namaste", "thanks", "thank you", "bye", "goodbye",
        ];
        greetings.iter().any(|g| query.contains(g))
    }

    fn is_acknowledgment(&self, query: &str, word_count: usize) -> bool {
        if word_count > 3 {
            return false;
        }
        let acks = ["ok", "okay", "yes", "no", "sure", "alright", "got it"];
        acks.iter().any(|a| query == *a || query.starts_with(a))
    }

    fn is_meta_question(&self, query: &str) -> bool {
        // Only match queries explicitly about the assistant itself
        // Queries with pronouns (this/that/it) should rely on context, not be meta
        let patterns = [
            "what is your name",
            "who are you",
            "what can you do",
            "how do you work",
            "what are your capabilities",
            "what context do you have",
            "what do you know about me",
            "what have we discussed",
            "what information do you have",
            "tell me about yourself",
            "what are you",
            "what's your role",
        ];
        patterns.iter().any(|p| query.contains(p))
    }

    fn is_clarification(&self, query: &str, word_count: usize) -> bool {
        if word_count > 10 {
            return false;
        }
        let patterns = ["what do you mean", "can you explain", "i don't understand", "clarify"];
        patterns.iter().any(|p| query.contains(p))
    }

    fn is_follow_up(&self, query: &str) -> bool {
        // Detect queries that reference previous context for re-formatting/re-displaying.
        // These should NOT trigger document search or web search.
        // The query must START with a transformation/display command
        // AND reference previous context with a pronoun/demonstrative.
        let starts_with_transform = [
            "show me", "display", "visualize", "format as", "convert to",
            "make it", "do the same", "repeat that", "do it again",
        ];
        let has_transform = starts_with_transform.iter().any(|p| query.starts_with(p));

        if !has_transform {
            return false;
        }

        // Must also have a context reference (pronoun/demonstrative)
        let context_words = [
            "the same", " that", " this", " those", " these", " it ", " them",
            " it.", // end of sentence
        ];
        context_words.iter().any(|w| query.contains(w))
    }

    fn is_creative_generation(&self, query: &str) -> bool {
        // Queries asking to create/generate fictional or example content.
        // IMPORTANT: "generate a report FROM my documents" is NOT creative — it needs retrieval.
        // So we exclude queries that reference existing documents/data.

        let doc_refs = ["from my", "from the", "from document", "from file", "based on my", "using my"];
        let references_docs = doc_refs.iter().any(|r| query.contains(r));
        if references_docs {
            return false;
        }

        let patterns = [
            "create fake", "create a fake", "make up", "invent", "imagine",
            "pretend", "fictional", "fabricate", "simulate", "mock up",
            "come up with", "brainstorm", "suggest some", "give me ideas",
            // Visualization/diagram creation (pure generation, no doc lookup needed)
            "make a flowchart", "make a diagram", "create a flowchart",
            "create a diagram", "draw a", "make a chart", "create a chart",
            "make an infographic",
        ];
        if patterns.iter().any(|p| query.contains(p)) {
            return true;
        }

        // "write a"/"draft a"/"compose a" are creative only when NOT about existing content
        let write_patterns = [
            "write a", "write an", "draft a", "draft an", "compose a",
        ];
        if write_patterns.iter().any(|p| query.starts_with(p)) {
            // Creative if not referencing documents
            return !query.contains("about my") && !query.contains("summary of");
        }

        false
    }

    fn is_example_creation(&self, query: &str) -> bool {
        // Queries asking for examples, samples, or test data
        let patterns = [
            "example of",
            "give me an example",
            "show me an example",
            "sample data",
            "show me a sample",
            "random data",
            "test data",
            "mock data",
            "dummy data",
            "placeholder",
            "demo data",
            "fake data",
            "synthetic data",
            "for testing",
            "for demo",
        ];
        patterns.iter().any(|p| query.contains(p))
    }

    fn is_calculation(&self, query: &str) -> bool {
        let lower = query.to_lowercase();
        // Explicit math keywords
        let has_math_keyword = lower.contains("calculate")
            || lower.contains("compute")
            || lower.contains("sum of")
            || lower.contains("average of")
            || lower.contains("multiply")
            || lower.contains("divide");

        // Pattern: "what is X + Y" with at least 2 numbers and an operator between them
        let has_arithmetic = {
            let num_count = query.chars().filter(|c| c.is_numeric()).count();
            num_count >= 2 && ARITHMETIC_RE.is_match(query)
        };

        has_math_keyword || has_arithmetic
    }

    fn is_filtered_search(&self, query: &str) -> bool {
        // Has comparison operators or filter keywords
        query.contains('>')
            || query.contains('<')
            || query.contains("greater than")
            || query.contains("less than")
            || query.contains("more than")
            || query.contains("exceeds")
            || query.contains("below")
            || query.contains("above")
            || (query.contains("where") && query.contains("and"))
            || (query.contains("with") && query.contains("than"))
    }

    fn is_comparative(&self, query: &str) -> bool {
        let patterns = [
            "compare",
            "difference between",
            "vs",
            "versus",
            "better than",
            "worse than",
            "similar to",
            "contrast",
        ];
        patterns.iter().any(|p| query.contains(p))
    }

    fn is_aggregation(&self, query: &str) -> bool {
        let patterns = [
            "how many",
            "count",
            "total",
            "sum",
            "average",
            "mean",
            "list all",
            "show all",
        ];
        patterns.iter().any(|p| query.contains(p))
    }

    fn is_temporal(&self, query: &str) -> bool {
        // Check for any 4-digit year pattern
        let has_year = YEAR_RE.is_match(query);

        if has_year { return true; }

        let patterns = [
            "last year",
            "this year",
            "last month",
            "this month",
            "last week",
            "this week",
            "recent",
            "latest",
            "during",
        ];
        patterns.iter().any(|p| query.contains(p))
    }

    fn is_multi_hop(&self, query: &str) -> bool {
        // Multiple explicit questions, or conditional reasoning chains
        let has_conditional = query.contains(" if ") && query.contains(" then ");
        let has_multi_question = query.matches('?').count() > 1;
        // "compare X and Y" or "difference between X and Y" implies multi-hop
        let has_comparison = query.contains("compare") || query.contains("difference between");
        has_conditional || has_multi_question || has_comparison
    }

    fn is_document_search(&self, query: &str) -> bool {
        let lower = query.to_lowercase();
        let patterns = [
            "find", "search", "show me", "get me", "retrieve", "fetch", "locate",
            "look up", "pull up", "bring up",
        ];
        patterns.iter().any(|p| lower.starts_with(p) || lower.contains(&format!(" {} ", p)))
    }

    fn is_definition(&self, query: &str) -> bool {
        // Don't match meta questions (already handled by is_meta_question)
        if query.contains("you") || query.contains("your") {
            return false;
        }

        let patterns = ["what is", "what are", "define", "explain", "tell me about"];
        patterns.iter().any(|p| query.starts_with(p))
    }

    fn is_document_specific(&self, query: &str) -> bool {
        // Contains terms that likely refer to documents in corpus
        let doc_terms = [
            "section",
            "clause",
            "provision",
            "article",
            "contract",
            "agreement",
            "document",
            "file",
            "paragraph",
            "page",
            "schedule",
            "annexure",
            "exhibit",
        ];
        doc_terms.iter().any(|t| query.contains(t))
    }

    fn is_current_events(&self, query: &str) -> bool {
        // Detect queries about news, current events, recent happenings
        // More specific than temporal queries - focuses on news/events

        // Don't trigger if query is about documents ("latest in my documents")
        if query.contains("document") || query.contains("file") || query.contains("my") {
            return false;
        }

        let patterns = [
            "news",
            "breaking",
            "headline",
            "happening now",
            "what's new in",
            "what's happening",
            "announcement",
            "current events",
            "recent events",
        ];
        patterns.iter().any(|p| query.contains(p))
    }

    fn is_realtime_info(&self, query: &str) -> bool {
        // Detect queries that need real-time/live data OR explicit web search requests
        let patterns = [
            "weather",
            "stock price",
            "live score",
            "right now",
            "currently",
            "at the moment",
            "real-time",
            "up-to-date",
            // Explicit web search requests
            "search online",
            "google",
            "search web",
            "look up online",
            "find online",
            "search internet",
            "web search",
        ];
        patterns.iter().any(|p| query.contains(p))
    }
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Domain Matcher
// ============================================================================

pub struct DomainMatcher {}

impl DomainMatcher {
    pub fn new() -> Self {
        Self {}
    }

    pub fn check_relevance(&self, query: &str, stats: &CorpusStats) -> RelevanceScore {
        let keywords = self.extract_keywords(query);

        if keywords.is_empty() {
            return RelevanceScore {
                corpus_coverage: 0.0,
                domain_match: 0.0,
                term_frequency: 0.0,
                overall_confidence: 0.0,
            };
        }

        // 1. Corpus coverage: how many query terms exist in corpus?
        let matching_terms = keywords
            .iter()
            .filter(|k| stats.vocabulary.contains(*k))
            .count();
        let corpus_coverage = matching_terms as f32 / keywords.len() as f32;

        // 2. Term frequency: are these common or rare terms?
        let term_frequencies: Vec<f32> = keywords
            .iter()
            .filter_map(|k| stats.domain_terms.get(k.as_str()))
            .copied()
            .collect();

        let term_frequency = if term_frequencies.is_empty() {
            0.0
        } else {
            term_frequencies.iter().sum::<f32>() / term_frequencies.len() as f32
        };

        // 3. Domain match: does query match document types in corpus?
        let domain_match = self.calculate_domain_match(query, stats);

        // 4. Overall confidence
        // Weight: coverage (40%), domain (40%), frequency (20%)
        let overall_confidence =
            (corpus_coverage * 0.4) + (domain_match * 0.4) + (term_frequency * 0.2);

        RelevanceScore {
            corpus_coverage,
            domain_match,
            term_frequency,
            overall_confidence,
        }
    }

    fn extract_keywords(&self, query: &str) -> Vec<String> {
        // Simple keyword extraction - remove stop words
        let stop_words = [
            "the", "a", "an", "is", "are", "was", "were", "in", "on", "at", "to", "for", "of",
            "and", "or", "but", "with", "from", "by", "as", "how", "what", "where", "when",
            "why", "which", "who", "i", "you", "me", "my", "your",
        ];

        query
            .to_lowercase()
            .split_whitespace()
            .filter(|w| !stop_words.contains(w) && w.len() > 2)
            .map(|w| w.to_string())
            .collect()
    }

    fn calculate_domain_match(&self, query: &str, stats: &CorpusStats) -> f32 {
        if stats.document_types.is_empty() {
            return 0.5; // Unknown, assume medium match
        }

        let query_lower = query.to_lowercase();

        // Legal domain indicators
        let legal_terms = [
            "contract",
            "agreement",
            "clause",
            "liability",
            "indemnity",
            "breach",
            "party",
            "provision",
            "termination",
        ];

        // Tax/GST domain indicators
        let tax_terms = [
            "gst",
            "tax",
            "return",
            "invoice",
            "itr",
            "section",
            "assessment",
            "audit",
            "compliance",
        ];

        // Check which domains are present in corpus
        let has_legal_docs = stats
            .document_types
            .keys()
            .any(|k| k.contains("contract") || k.contains("agreement") || k.contains("legal"));

        let has_tax_docs = stats
            .document_types
            .keys()
            .any(|k| k.contains("gst") || k.contains("tax") || k.contains("itr"));

        // Check if query matches available domains
        let query_is_legal = legal_terms.iter().any(|t| query_lower.contains(t));
        let query_is_tax = tax_terms.iter().any(|t| query_lower.contains(t));

        if (query_is_legal && has_legal_docs) || (query_is_tax && has_tax_docs) {
            0.9
        } else if query_is_legal && !has_legal_docs {
            0.1 // Query about legal but we don't have legal docs
        } else if query_is_tax && !has_tax_docs {
            0.1 // Query about tax but we don't have tax docs
        } else {
            0.5 // Neutral - can't determine domain mismatch
        }
    }
}

impl Default for DomainMatcher {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Requirement Extractor
// ============================================================================

pub struct RequirementExtractor {}

impl RequirementExtractor {
    pub fn new() -> Self {
        Self {}
    }

    pub fn extract(&self, query: &str) -> QueryRequirements {
        QueryRequirements {
            needs_filtering: self.detect_filtering(query),
            date_range: self.extract_date_range(query),
            numeric_conditions: self.extract_numeric_conditions(query),
            entity_references: self.extract_entities(query),
            document_type_hints: self.extract_document_types(query),
        }
    }

    fn detect_filtering(&self, query: &str) -> bool {
        query.contains('>')
            || query.contains('<')
            || query.contains("where")
            || query.contains("greater than")
            || query.contains("less than")
    }

    fn extract_date_range(&self, query: &str) -> Option<DateRange> {
        let query_lower = query.to_lowercase();

        // Simple year detection
        if query_lower.contains("2023") {
            return Some(DateRange {
                field: "date".to_string(),
                start: Some("2023-01-01".to_string()),
                end: Some("2023-12-31".to_string()),
            });
        }

        if query_lower.contains("2024") {
            return Some(DateRange {
                field: "date".to_string(),
                start: Some("2024-01-01".to_string()),
                end: Some("2024-12-31".to_string()),
            });
        }

        // Could expand to handle "last month", "this year", etc.
        None
    }

    fn extract_numeric_conditions(&self, query: &str) -> Vec<NumericCondition> {
        let mut conditions = Vec::new();

        // Look for patterns like "> 90 days", "payment terms > 90"
        if query.contains('>') {
            // Extract number after >
            if let Some(num) = self.extract_number_after(query, '>') {
                conditions.push(NumericCondition {
                    field: "value".to_string(), // Generic field
                    operator: ComparisonOp::GreaterThan,
                    value: num,
                });
            }
        }

        if query.contains('<') {
            if let Some(num) = self.extract_number_after(query, '<') {
                conditions.push(NumericCondition {
                    field: "value".to_string(),
                    operator: ComparisonOp::LessThan,
                    value: num,
                });
            }
        }

        conditions
    }

    fn extract_number_after(&self, text: &str, symbol: char) -> Option<f64> {
        if let Some(pos) = text.find(symbol) {
            let after = &text[pos + 1..];
            let num_str: String = after
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| c.is_numeric() || *c == '.')
                .collect();

            num_str.parse::<f64>().ok()
        } else {
            None
        }
    }

    fn extract_entities(&self, query: &str) -> Vec<String> {
        // Simple entity extraction - capitalized words
        query
            .split_whitespace()
            .filter(|w| w.chars().next().map_or(false, |c| c.is_uppercase()) && w.len() > 2)
            .map(|s| s.to_string())
            .collect()
    }

    fn extract_document_types(&self, query: &str) -> Vec<String> {
        let query_lower = query.to_lowercase();
        let mut doc_types = Vec::new();

        let type_keywords = [
            ("contract", "contract"),
            ("agreement", "agreement"),
            ("invoice", "invoice"),
            ("gst return", "gst_return"),
            ("itr", "itr"),
            ("receipt", "receipt"),
            ("bill", "bill"),
        ];

        for (keyword, doc_type) in &type_keywords {
            if query_lower.contains(keyword) {
                doc_types.push(doc_type.to_string());
            }
        }

        doc_types
    }
}

impl Default for RequirementExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Strategy Selector
// ============================================================================

pub struct StrategySelector {}

impl StrategySelector {
    pub fn new() -> Self {
        Self {}
    }

    pub fn decide(
        &self,
        query: &str,
        intent: &QueryIntent,
        relevance: &RelevanceScore,
        requirements: &QueryRequirements,
        stats: &CorpusStats,
    ) -> RetrievalDecision {
        // Decision tree based on intent

        match intent {
            QueryIntent::Greeting | QueryIntent::SimpleAcknowledgment => {
                self.no_retrieval("Greeting or acknowledgment - no documents needed", false)
            }

            QueryIntent::MetaQuestion | QueryIntent::Clarification => {
                self.no_retrieval("Meta question about assistant capabilities", true)
            }

            QueryIntent::FollowUpRequest => {
                self.no_retrieval("Follow-up request referencing previous context - no new documents needed", true)
            }

            QueryIntent::Calculation => {
                self.no_retrieval("Mathematical calculation - LLM can answer directly", true)
            }

            QueryIntent::CreativeGeneration | QueryIntent::ExampleCreation => {
                self.no_retrieval("Creative/generative query - LLM can create content directly without documents", true)
            }

            QueryIntent::GeneralKnowledge => {
                // CRITICAL: Check if query terms exist in corpus vocabulary FIRST
                // Don't rely solely on confidence score - if documents contain the terms, search locally
                if relevance.corpus_coverage > 0.3 || stats.total_docs > 0 {
                    // Query terms found in corpus OR we have documents - search locally
                    let k = if relevance.overall_confidence > 0.5 { 5 } else { 10 };
                    self.simple_retrieval(k, relevance.overall_confidence.max(0.6), "General query - searching local documents first")
                } else if relevance.overall_confidence < 0.2 && stats.total_docs == 0 {
                    // Empty corpus or very low relevance - try web search
                    RetrievalDecision {
                        should_retrieve: true,
                        strategy: RetrievalStrategy::WebSearch {
                            query: query.to_string(),
                            max_results: 10,
                        },
                        estimated_docs_needed: 10,
                        confidence: 0.85,
                        reasoning: "No local documents or query terms not in vocabulary - searching web".to_string(),
                        fallback_plan: Some("Will search local documents if web search fails".to_string()),
                    }
                } else {
                    // Medium confidence - try local first
                    self.simple_retrieval(10, relevance.overall_confidence.max(0.5), "General query - checking local documents")
                }
            }

            QueryIntent::FactualLookup | QueryIntent::DefinitionQuery => {
                // Check if query terms exist in local documents first
                if relevance.corpus_coverage > 0.3 || stats.total_docs > 0 {
                    // Query terms found OR we have documents - search locally first
                    let k = if relevance.overall_confidence > 0.7 { 5 } else { 10 };
                    self.simple_retrieval(k, relevance.overall_confidence.max(0.6), "Factual lookup in local documents")
                } else if stats.total_docs == 0 {
                    // No local documents - use web search
                    RetrievalDecision {
                        should_retrieve: true,
                        strategy: RetrievalStrategy::WebSearch {
                            query: query.to_string(),
                            max_results: 10,
                        },
                        estimated_docs_needed: 10,
                        confidence: 0.85,
                        reasoning: "No local documents - searching web for factual information".to_string(),
                        fallback_plan: Some("LLM can provide general knowledge if web search fails".to_string()),
                    }
                } else {
                    // Have documents but low confidence - still search locally
                    self.simple_retrieval(10, relevance.overall_confidence.max(0.5), "Factual lookup - checking local documents")
                }
            }

            QueryIntent::DocumentSearch => {
                // DocumentSearch should always try to search if we have any documents
                if stats.total_docs > 0 {
                    // We have documents - search them
                    let k = if relevance.overall_confidence > 0.6 { 10 } else { 20 };
                    self.simple_retrieval(k, relevance.overall_confidence.max(0.6), "Searching indexed documents")
                } else {
                    // No documents to search
                    self.no_retrieval_with_fallback(
                        "No documents indexed in this space",
                        "Please index documents before searching."
                    )
                }
            }

            QueryIntent::FilteredSearch => {
                if relevance.overall_confidence < 0.4 {
                    self.no_retrieval_with_fallback(
                        "Filtered search but low corpus relevance",
                        "Your documents may not have the required fields or data."
                    )
                } else {
                    self.filtered_search(requirements, relevance.overall_confidence)
                }
            }

            QueryIntent::ComparativeAnalysis => {
                if stats.total_docs < 2 {
                    self.no_retrieval_with_fallback(
                        "Comparative analysis needs at least 2 documents",
                        "Add more documents to enable comparisons."
                    )
                } else {
                    self.multi_stage_retrieval(query, relevance.overall_confidence)
                }
            }

            QueryIntent::AggregationQuery => {
                if relevance.overall_confidence < 0.5 {
                    self.no_retrieval_with_fallback(
                        "Aggregation query but low corpus relevance",
                        "Your documents may not contain the data needed for aggregation."
                    )
                } else {
                    // Need to retrieve many docs for aggregation
                    let k = (stats.total_docs as f32 * 0.5).min(100.0) as usize;
                    RetrievalDecision {
                        should_retrieve: true,
                        strategy: RetrievalStrategy::FilteredSearch {
                            initial_k: k.max(20),
                            filters: vec!["Aggregate after retrieval".to_string()],
                        },
                        estimated_docs_needed: k,
                        confidence: relevance.overall_confidence,
                        reasoning: "Aggregation requires retrieving multiple documents".to_string(),
                        fallback_plan: Some("Results will be aggregated across all matching documents".to_string()),
                    }
                }
            }

            QueryIntent::TemporalQuery => {
                if requirements.date_range.is_some() {
                    self.filtered_search(requirements, relevance.overall_confidence)
                } else {
                    self.simple_retrieval(15, relevance.overall_confidence, "Temporal query without explicit dates")
                }
            }

            QueryIntent::MultiHopReasoning => {
                if relevance.overall_confidence < 0.5 {
                    self.no_retrieval_with_fallback(
                        "Multi-hop reasoning query but unclear corpus relevance",
                        "Simplify your question or verify your documents contain the needed information."
                    )
                } else {
                    self.multi_stage_retrieval(query, relevance.overall_confidence)
                }
            }

            QueryIntent::CurrentEvents | QueryIntent::RealTimeInfo => {
                // Current events/real-time info always needs web search
                // Check if local docs also relevant (hybrid) or web only
                if relevance.overall_confidence > 0.6 {
                    // High local relevance: combine local + web
                    RetrievalDecision {
                        should_retrieve: true,
                        strategy: RetrievalStrategy::HybridSearch {
                            local_k: 5,
                            web_results: 5,
                        },
                        estimated_docs_needed: 10,
                        confidence: 0.9,
                        reasoning: "Hybrid search: combining your documents with current web information".to_string(),
                        fallback_plan: None,
                    }
                } else {
                    // Low local relevance: web only
                    RetrievalDecision {
                        should_retrieve: true,
                        strategy: RetrievalStrategy::WebSearch {
                            query: query.to_string(),
                            max_results: 10,
                        },
                        estimated_docs_needed: 10,
                        confidence: 0.95,
                        reasoning: "Web search needed for current/real-time information".to_string(),
                        fallback_plan: None,
                    }
                }
            }
        }
    }

    fn no_retrieval(&self, reason: &str, llm_can_answer: bool) -> RetrievalDecision {
        RetrievalDecision {
            should_retrieve: false,
            strategy: RetrievalStrategy::NoRetrieval {
                reason: reason.to_string(),
                llm_can_answer,
            },
            estimated_docs_needed: 0,
            confidence: 1.0,
            reasoning: reason.to_string(),
            fallback_plan: None,
        }
    }

    fn no_retrieval_with_fallback(&self, reason: &str, fallback: &str) -> RetrievalDecision {
        RetrievalDecision {
            should_retrieve: false,
            strategy: RetrievalStrategy::NoRetrieval {
                reason: reason.to_string(),
                llm_can_answer: false,
            },
            estimated_docs_needed: 0,
            confidence: 0.8,
            reasoning: reason.to_string(),
            fallback_plan: Some(fallback.to_string()),
        }
    }

    fn simple_retrieval(&self, k: usize, confidence: f32, reason: &str) -> RetrievalDecision {
        RetrievalDecision {
            should_retrieve: true,
            strategy: RetrievalStrategy::TopK { k },
            estimated_docs_needed: k,
            confidence,
            reasoning: format!("Simple top-{} retrieval: {}", k, reason),
            fallback_plan: None,
        }
    }

    fn filtered_search(&self, requirements: &QueryRequirements, confidence: f32) -> RetrievalDecision {
        let mut filters = Vec::new();

        if let Some(ref date_range) = requirements.date_range {
            filters.push(format!("Date range: {} to {}",
                date_range.start.as_ref().unwrap_or(&"?".to_string()),
                date_range.end.as_ref().unwrap_or(&"?".to_string())
            ));
        }

        for condition in &requirements.numeric_conditions {
            filters.push(format!("{} {:?} {}", condition.field, condition.operator, condition.value));
        }

        let initial_k = if filters.is_empty() { 20 } else { 50 };

        RetrievalDecision {
            should_retrieve: true,
            strategy: RetrievalStrategy::FilteredSearch {
                initial_k,
                filters: filters.clone(),
            },
            estimated_docs_needed: initial_k,
            confidence,
            reasoning: format!("Filtered search with {} conditions", filters.len()),
            fallback_plan: Some("Results will be filtered after retrieval".to_string()),
        }
    }

    fn multi_stage_retrieval(&self, query: &str, confidence: f32) -> RetrievalDecision {
        // For complex queries, plan multiple search stages
        let stages = vec![
            SearchStage {
                stage_name: "Initial broad search".to_string(),
                search_query: query.to_string(),
                max_results: 20,
            },
            SearchStage {
                stage_name: "Refined search on results".to_string(),
                search_query: "Refine based on initial results".to_string(),
                max_results: 10,
            },
        ];

        RetrievalDecision {
            should_retrieve: true,
            strategy: RetrievalStrategy::MultiStage { stages: stages.clone() },
            estimated_docs_needed: 30,
            confidence,
            reasoning: format!("Multi-stage retrieval with {} stages", stages.len()),
            fallback_plan: Some("Multiple search iterations for complex query".to_string()),
        }
    }
}

impl Default for StrategySelector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_greeting_detection() {
        let classifier = IntentClassifier::new();
        assert_eq!(classifier.classify("hello"), QueryIntent::Greeting);
        assert_eq!(classifier.classify("thanks"), QueryIntent::Greeting);
    }

    #[test]
    fn test_filtered_search_detection() {
        let classifier = IntentClassifier::new();
        let intent = classifier.classify("find contracts where payment > 90 days");
        assert_eq!(intent, QueryIntent::FilteredSearch);
    }

    #[test]
    fn test_comparative_detection() {
        let classifier = IntentClassifier::new();
        let intent = classifier.classify("compare contract A vs contract B");
        assert_eq!(intent, QueryIntent::ComparativeAnalysis);
    }

    #[test]
    fn test_domain_relevance() {
        let matcher = DomainMatcher::new();
        let mut stats = CorpusStats::default();
        stats.vocabulary.insert("contract".to_string());
        stats.vocabulary.insert("liability".to_string());
        stats.domain_terms.insert("contract".to_string(), 0.8);
        stats.domain_terms.insert("liability".to_string(), 0.6);

        let relevance = matcher.check_relevance("find liability in contract", &stats);
        assert!(relevance.corpus_coverage > 0.5);
    }

    #[test]
    fn test_numeric_extraction() {
        let extractor = RequirementExtractor::new();
        let requirements = extractor.extract("payment > 90 days");
        assert!(requirements.needs_filtering);
        assert_eq!(requirements.numeric_conditions.len(), 1);
        assert_eq!(requirements.numeric_conditions[0].value, 90.0);
    }
}
