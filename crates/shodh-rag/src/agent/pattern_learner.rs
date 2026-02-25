//! Pattern Learning Engine - Learns from user behavior to make intelligent suggestions

use anyhow::Result;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{Activity, ActivityType, Suggestion};

/// Pattern learner that identifies user habits and preferences
pub struct PatternLearner {
    // Pattern storage
    time_patterns: Arc<RwLock<TimePatterns>>,
    sequence_patterns: Arc<RwLock<SequencePatterns>>,
    context_patterns: Arc<RwLock<ContextPatterns>>,

    // Learning state
    learning_buffer: Arc<RwLock<VecDeque<Activity>>>,
    pattern_confidence: Arc<RwLock<HashMap<String, f32>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TimePatterns {
    /// Activities by hour of day
    hourly_activities: HashMap<u32, Vec<String>>,

    /// Activities by day of week
    daily_activities: HashMap<u32, Vec<String>>,

    /// Project preferences by time
    time_project_preferences: HashMap<u32, HashMap<String, f32>>,

    /// Most productive hours
    productive_hours: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SequencePatterns {
    /// Common activity sequences (n-grams)
    bigrams: HashMap<String, HashMap<String, f32>>,
    trigrams: HashMap<String, HashMap<String, f32>>,

    /// Project transition patterns
    project_transitions: HashMap<String, HashMap<String, f32>>,

    /// Task completion patterns
    completion_patterns: Vec<CompletionPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CompletionPattern {
    trigger: String,
    actions: Vec<String>,
    confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ContextPatterns {
    /// File access patterns
    file_patterns: HashMap<String, Vec<String>>,

    /// Search patterns
    search_patterns: HashMap<String, Vec<String>>,

    /// Tool usage patterns
    tool_patterns: HashMap<String, f32>,
}

/// Click pattern data for personalized search ranking
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClickPatternData {
    /// Number of clicks per result ID
    pub click_counts: HashMap<String, usize>,

    /// Number of ignores per result ID
    pub ignore_counts: HashMap<String, usize>,

    /// Dwell times per result ID (in seconds)
    pub dwell_times: HashMap<String, Vec<u64>>,
}

impl PatternLearner {
    pub fn new() -> Result<Self> {
        Ok(Self {
            time_patterns: Arc::new(RwLock::new(TimePatterns::default())),
            sequence_patterns: Arc::new(RwLock::new(SequencePatterns::default())),
            context_patterns: Arc::new(RwLock::new(ContextPatterns::default())),
            learning_buffer: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            pattern_confidence: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Learn from a new activity
    pub async fn learn_from_activity(&self, activity: &Activity) -> Result<()> {
        // Add to buffer
        let mut buffer = self.learning_buffer.write().await;
        if buffer.len() >= 1000 {
            buffer.pop_front();
        }
        buffer.push_back(activity.clone());
        drop(buffer);

        // Learn time patterns
        self.learn_time_pattern(activity).await?;

        // Learn sequence patterns
        self.learn_sequence_pattern(activity).await?;

        // Learn context patterns
        self.learn_context_pattern(activity).await?;

        // Update confidence scores
        self.update_confidence().await?;

        Ok(())
    }

    /// Get time-based suggestions
    pub async fn get_time_based_suggestions(&self) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();
        let current_hour = Utc::now().hour();
        let current_day = Utc::now().weekday().num_days_from_monday();

        let time_patterns = self.time_patterns.read().await;

        // Get activities commonly done at this hour
        if let Some(hourly) = time_patterns.hourly_activities.get(&current_hour) {
            for activity in hourly.iter().take(3) {
                suggestions.push(Suggestion {
                    text: format!("Continue with: {}", activity),
                    confidence: 0.7,
                    reason: format!("You usually do this at {}:00", current_hour),
                });
            }
        }

        // Get projects commonly worked on at this time
        if let Some(projects) = time_patterns.time_project_preferences.get(&current_hour) {
            for (project, confidence) in projects.iter().take(2) {
                suggestions.push(Suggestion {
                    text: format!("Work on project: {}", project),
                    confidence: *confidence,
                    reason: "Based on your time preferences".to_string(),
                });
            }
        }

        // Productivity suggestions
        if time_patterns.productive_hours.contains(&current_hour) {
            suggestions.push(Suggestion {
                text: "This is your productive time - tackle complex tasks".to_string(),
                confidence: 0.8,
                reason: "Historical productivity peak".to_string(),
            });
        }

        Ok(suggestions)
    }

    /// Get pattern-based suggestions
    pub async fn get_pattern_suggestions(&self) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();

        // Get last activity
        let buffer = self.learning_buffer.read().await;
        if let Some(last_activity) = buffer.back() {
            // Get sequence predictions
            let sequence_suggestions = self.predict_next_activity(last_activity).await?;
            suggestions.extend(sequence_suggestions);

            // Get context predictions
            let context_suggestions = self.predict_from_context(last_activity).await?;
            suggestions.extend(context_suggestions);
        }

        Ok(suggestions)
    }

    /// Predict what the user will search for
    pub async fn predict_search(&self, partial: &str) -> Result<Vec<String>> {
        let context_patterns = self.context_patterns.read().await;
        let mut predictions = Vec::new();

        // Find matching search patterns
        for (pattern, completions) in &context_patterns.search_patterns {
            if pattern.starts_with(partial) {
                predictions.extend(completions.clone());
            }
        }

        // Sort by frequency (would need frequency tracking)
        predictions.dedup();
        predictions.truncate(5);

        Ok(predictions)
    }

    /// Get file suggestions based on current context
    pub async fn suggest_files(&self, context: &str) -> Result<Vec<String>> {
        let context_patterns = self.context_patterns.read().await;

        if let Some(files) = context_patterns.file_patterns.get(context) {
            Ok(files.clone())
        } else {
            Ok(vec![])
        }
    }

    /// Get click patterns for a specific query
    pub async fn get_click_patterns_for_query(&self, query: &str) -> Result<ClickPatternData> {
        let patterns = self.context_patterns.read().await;

        let mut click_counts: HashMap<String, usize> = HashMap::new();
        let mut ignore_counts: HashMap<String, usize> = HashMap::new();
        let mut dwell_times: HashMap<String, Vec<u64>> = HashMap::new();

        // Get clicks for this query
        let click_key = format!("click:{}", query);
        if let Some(click_patterns) = patterns.search_patterns.get(&click_key) {
            for pattern_str in click_patterns {
                if let Ok(pattern_data) = serde_json::from_str::<serde_json::Value>(pattern_str) {
                    if let Some(result_id) = pattern_data["result_id"].as_str() {
                        *click_counts.entry(result_id.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }

        // Get ignores for this query
        let ignore_key = format!("ignore:{}", query);
        if let Some(ignore_patterns) = patterns.search_patterns.get(&ignore_key) {
            for pattern_str in ignore_patterns {
                if let Ok(pattern_data) = serde_json::from_str::<serde_json::Value>(pattern_str) {
                    if let Some(result_id) = pattern_data["result_id"].as_str() {
                        *ignore_counts.entry(result_id.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }

        // Get dwell times for results
        for (key, dwell_patterns) in patterns.file_patterns.iter() {
            if let Some(result_id) = key.strip_prefix("dwell:") {
                for pattern_str in dwell_patterns {
                    if let Ok(pattern_data) = serde_json::from_str::<serde_json::Value>(pattern_str)
                    {
                        if let Some(dwell_time) = pattern_data["dwell_time_seconds"].as_u64() {
                            dwell_times
                                .entry(result_id.to_string())
                                .or_insert_with(Vec::new)
                                .push(dwell_time);
                        }
                    }
                }
            }
        }

        Ok(ClickPatternData {
            click_counts,
            ignore_counts,
            dwell_times,
        })
    }

    /// Calculate personalization boost for a search result based on learned patterns
    pub async fn calculate_personalization_boost(
        &self,
        result_id: &str,
        query: &str,
    ) -> Result<f32> {
        let pattern_data = self.get_click_patterns_for_query(query).await?;

        let mut boost = 0.0;

        // Boost for clicks (positive signal)
        if let Some(click_count) = pattern_data.click_counts.get(result_id) {
            boost += (*click_count as f32) * 0.3;
        }

        // Penalty for ignores (negative signal)
        if let Some(ignore_count) = pattern_data.ignore_counts.get(result_id) {
            boost -= (*ignore_count as f32) * 0.2;
        }

        // Boost for high dwell time (engagement signal)
        if let Some(dwell_times) = pattern_data.dwell_times.get(result_id) {
            if !dwell_times.is_empty() {
                let avg_dwell: f32 =
                    dwell_times.iter().sum::<u64>() as f32 / dwell_times.len() as f32;
                // Boost if dwell time > 10 seconds (indicates engagement)
                if avg_dwell > 10.0 {
                    boost += 0.15;
                }
            }
        }

        Ok(boost.max(-0.5).min(1.0)) // Clamp between -0.5 and 1.0
    }

    /// Get all click patterns for debugging/insights
    pub async fn get_all_click_patterns(&self) -> Result<HashMap<String, ClickPatternData>> {
        let patterns = self.context_patterns.read().await;
        let mut all_patterns = HashMap::new();

        // Extract unique queries from click patterns
        let mut queries = std::collections::HashSet::new();
        for key in patterns.search_patterns.keys() {
            if let Some(query) = key.strip_prefix("click:") {
                queries.insert(query.to_string());
            }
        }

        // Get patterns for each query
        for query in queries {
            if let Ok(pattern_data) = self.get_click_patterns_for_query(&query).await {
                all_patterns.insert(query, pattern_data);
            }
        }

        Ok(all_patterns)
    }

    // Private learning methods

    async fn learn_time_pattern(&self, activity: &Activity) -> Result<()> {
        let mut patterns = self.time_patterns.write().await;
        let hour = activity.timestamp.hour();
        let day = activity.timestamp.weekday().num_days_from_monday();

        // Record hourly activity
        let activity_name = self.activity_name(activity);
        patterns
            .hourly_activities
            .entry(hour)
            .or_insert_with(Vec::new)
            .push(activity_name.clone());

        // Record daily activity
        patterns
            .daily_activities
            .entry(day)
            .or_insert_with(Vec::new)
            .push(activity_name);

        // Record project time preferences
        if let Some(ref project) = activity.project {
            let project_prefs = patterns
                .time_project_preferences
                .entry(hour)
                .or_insert_with(HashMap::new);

            let current = project_prefs.get(project).unwrap_or(&0.0);
            project_prefs.insert(project.clone(), (current + 0.1).min(1.0));
        }

        // Update productive hours (simplified - would need more complex analysis)
        if matches!(activity.activity_type, ActivityType::TaskCompleted(_)) {
            if !patterns.productive_hours.contains(&hour) {
                patterns.productive_hours.push(hour);
            }
        }

        Ok(())
    }

    async fn learn_sequence_pattern(&self, activity: &Activity) -> Result<()> {
        let buffer = self.learning_buffer.read().await;

        if buffer.len() < 2 {
            return Ok(());
        }

        let mut patterns = self.sequence_patterns.write().await;

        // Learn bigrams (pairs)
        if buffer.len() >= 2 {
            let prev = &buffer[buffer.len() - 2];
            let prev_name = self.activity_name(prev);
            let curr_name = self.activity_name(activity);

            let bigram_entry = patterns
                .bigrams
                .entry(prev_name.clone())
                .or_insert_with(HashMap::new);

            let current = bigram_entry.get(&curr_name).unwrap_or(&0.0);
            bigram_entry.insert(curr_name.clone(), (current + 0.1).min(1.0));
        }

        // Learn trigrams (triples)
        if buffer.len() >= 3 {
            let prev2 = &buffer[buffer.len() - 3];
            let prev1 = &buffer[buffer.len() - 2];
            let key = format!(
                "{}_{}",
                self.activity_name(prev2),
                self.activity_name(prev1)
            );
            let curr_name = self.activity_name(activity);

            let trigram_entry = patterns.trigrams.entry(key).or_insert_with(HashMap::new);

            let current = trigram_entry.get(&curr_name).unwrap_or(&0.0);
            trigram_entry.insert(curr_name, (current + 0.1).min(1.0));
        }

        // Learn project transitions
        if buffer.len() >= 2 {
            let prev = &buffer[buffer.len() - 2];
            if let (Some(prev_proj), Some(curr_proj)) = (&prev.project, &activity.project) {
                if prev_proj != curr_proj {
                    let transitions = patterns
                        .project_transitions
                        .entry(prev_proj.clone())
                        .or_insert_with(HashMap::new);

                    let current = transitions.get(curr_proj).unwrap_or(&0.0);
                    transitions.insert(curr_proj.clone(), (current + 0.1).min(1.0));
                }
            }
        }

        Ok(())
    }

    async fn learn_context_pattern(&self, activity: &Activity) -> Result<()> {
        let mut patterns = self.context_patterns.write().await;

        match &activity.activity_type {
            ActivityType::FileEdited(file) => {
                if let Some(project) = &activity.project {
                    patterns
                        .file_patterns
                        .entry(project.clone())
                        .or_insert_with(Vec::new)
                        .push(file.clone());
                }
            }
            ActivityType::Search(query) => {
                let words: Vec<&str> = query.split_whitespace().collect();
                if !words.is_empty() {
                    let prefix = words[0].to_string();
                    patterns
                        .search_patterns
                        .entry(prefix)
                        .or_insert_with(Vec::new)
                        .push(query.clone());
                }
            }
            ActivityType::CommandExecuted(cmd) => {
                let tool = cmd.split_whitespace().next().unwrap_or("unknown");
                let current = *patterns.tool_patterns.get(tool).unwrap_or(&0.0);
                patterns
                    .tool_patterns
                    .insert(tool.to_string(), (current + 0.1).min(1.0));
            }
            ActivityType::ResultClicked {
                result_id,
                query,
                rank,
                score,
            } => {
                let key = format!("click:{}", query);
                let pattern_data = serde_json::json!({
                    "result_id": result_id,
                    "rank": rank,
                    "score": score,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                patterns
                    .search_patterns
                    .entry(key)
                    .or_insert_with(Vec::new)
                    .push(pattern_data.to_string());
                tracing::debug!(query = %query, result_id = %result_id, rank = rank, "Learned click pattern");
            }
            ActivityType::ResultViewed {
                result_id,
                dwell_time_seconds,
            } => {
                let key = format!("dwell:{}", result_id);
                let pattern_data = serde_json::json!({
                    "dwell_time_seconds": dwell_time_seconds,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                patterns
                    .file_patterns
                    .entry(key)
                    .or_insert_with(Vec::new)
                    .push(pattern_data.to_string());
                tracing::debug!(result_id = %result_id, dwell_time_seconds = dwell_time_seconds, "Learned dwell time pattern");
            }
            ActivityType::ResultIgnored {
                result_id,
                query,
                rank,
            } => {
                let key = format!("ignore:{}", query);
                let pattern_data = serde_json::json!({
                    "result_id": result_id,
                    "rank": rank,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                patterns
                    .search_patterns
                    .entry(key)
                    .or_insert_with(Vec::new)
                    .push(pattern_data.to_string());
                tracing::debug!(query = %query, result_id = %result_id, rank = rank, "Learned ignore pattern");
            }
            _ => {}
        }

        Ok(())
    }

    async fn predict_next_activity(&self, last: &Activity) -> Result<Vec<Suggestion>> {
        let patterns = self.sequence_patterns.read().await;
        let mut suggestions = Vec::new();

        let last_name = self.activity_name(last);

        // Check bigram predictions
        if let Some(next_activities) = patterns.bigrams.get(&last_name) {
            for (activity, confidence) in next_activities.iter() {
                suggestions.push(Suggestion {
                    text: format!("Next: {}", activity),
                    confidence: *confidence,
                    reason: "Based on your activity patterns".to_string(),
                });
            }
        }

        // Check project transitions
        if let Some(project) = &last.project {
            if let Some(transitions) = patterns.project_transitions.get(project) {
                for (next_project, confidence) in transitions.iter().take(1) {
                    suggestions.push(Suggestion {
                        text: format!("Switch to project: {}", next_project),
                        confidence: *confidence,
                        reason: "Common project transition".to_string(),
                    });
                }
            }
        }

        Ok(suggestions)
    }

    async fn predict_from_context(&self, activity: &Activity) -> Result<Vec<Suggestion>> {
        let patterns = self.context_patterns.read().await;
        let mut suggestions = Vec::new();

        // Suggest frequently used tools
        let mut tool_suggestions: Vec<_> = patterns
            .tool_patterns
            .iter()
            .filter(|(_, conf)| **conf > 0.5)
            .collect();

        tool_suggestions.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));

        for (tool, confidence) in tool_suggestions.iter().take(2) {
            suggestions.push(Suggestion {
                text: format!("Use tool: {}", tool),
                confidence: **confidence,
                reason: "Frequently used tool".to_string(),
            });
        }

        Ok(suggestions)
    }

    async fn update_confidence(&self) -> Result<()> {
        // Update pattern confidence based on consistency
        // This would involve statistical analysis of pattern reliability
        Ok(())
    }

    fn activity_name(&self, activity: &Activity) -> String {
        match &activity.activity_type {
            ActivityType::FileEdited(f) => format!("Edit:{}", f.split('/').last().unwrap_or(f)),
            ActivityType::Search(q) => format!("Search:{}", q),
            ActivityType::DocumentAdded(d) => format!("Add:{}", d),
            ActivityType::TaskCompleted(t) => format!("Complete:{}", t),
            ActivityType::CommandExecuted(c) => {
                format!("Cmd:{}", c.split_whitespace().next().unwrap_or(c))
            }
            ActivityType::ProjectSwitched(p) => format!("Switch:{}", p),
            ActivityType::ResultClicked { result_id, .. } => format!("Click:{}", result_id),
            ActivityType::ResultViewed {
                result_id,
                dwell_time_seconds,
            } => format!("View:{}:{}s", result_id, dwell_time_seconds),
            ActivityType::ResultIgnored { result_id, .. } => format!("Ignore:{}", result_id),
        }
    }
}
