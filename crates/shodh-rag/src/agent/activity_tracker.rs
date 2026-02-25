//! Activity Tracking - Records and analyzes user activities within the agent framework

use anyhow::Result;
use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ============================================================================
// Core Activity Types
// ============================================================================

/// User activity record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub activity_type: ActivityType,
    pub project: Option<String>,
    pub metadata: serde_json::Value,
}

/// Types of activities that can be tracked
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActivityType {
    FileEdited(String),
    Search(String),
    DocumentAdded(String),
    TaskCompleted(String),
    CommandExecuted(String),
    ProjectSwitched(String),
    // Click tracking for personalization
    ResultClicked {
        result_id: String,
        query: String,
        rank: usize,
        score: f32,
    },
    ResultViewed {
        result_id: String,
        dwell_time_seconds: u64,
    },
    ResultIgnored {
        result_id: String,
        query: String,
        rank: usize,
    },
}

// ============================================================================
// Activity Tracker
// ============================================================================

/// Activity tracker for recording user actions
pub struct ActivityTracker {
    // In-memory storage (would be backed by DB in production)
    activities: Arc<RwLock<VecDeque<Activity>>>,

    // Indexes for fast lookups
    by_date: Arc<RwLock<HashMap<NaiveDate, Vec<Uuid>>>>,
    by_project: Arc<RwLock<HashMap<String, Vec<Uuid>>>>,
    by_type: Arc<RwLock<HashMap<String, Vec<Uuid>>>>,

    // Statistics
    stats: Arc<RwLock<ActivityStats>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityStats {
    pub total_activities: usize,
    pub activities_today: usize,
    pub most_active_hour: Option<u32>,
    pub most_used_project: Option<String>,
    pub common_searches: Vec<String>,
}

impl ActivityTracker {
    pub fn new() -> Result<Self> {
        Ok(Self {
            activities: Arc::new(RwLock::new(VecDeque::with_capacity(10000))),
            by_date: Arc::new(RwLock::new(HashMap::new())),
            by_project: Arc::new(RwLock::new(HashMap::new())),
            by_type: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(ActivityStats::default())),
        })
    }

    /// Record a new activity
    pub async fn record(&self, mut activity: Activity) -> Result<()> {
        // Assign ID if not present
        if activity.id == Uuid::nil() {
            activity.id = Uuid::new_v4();
        }

        // Add to main storage
        let mut activities = self.activities.write().await;

        // Limit storage size
        if activities.len() >= 10000 {
            activities.pop_front();
        }

        activities.push_back(activity.clone());
        drop(activities);

        // Update indexes
        self.update_indexes(&activity).await?;

        // Update statistics
        self.update_stats(&activity).await?;

        Ok(())
    }

    /// Get recent activities
    pub async fn get_recent_activities(&self, limit: usize) -> Result<Vec<Activity>> {
        let activities = self.activities.read().await;
        Ok(activities.iter().rev().take(limit).cloned().collect())
    }

    /// Get activities for a specific date
    pub async fn get_activities_for_date(&self, date: NaiveDate) -> Result<Vec<Activity>> {
        let by_date = self.by_date.read().await;
        let activities = self.activities.read().await;

        if let Some(ids) = by_date.get(&date) {
            Ok(ids
                .iter()
                .filter_map(|id| activities.iter().find(|a| a.id == *id))
                .cloned()
                .collect())
        } else {
            Ok(vec![])
        }
    }

    /// Get last session activities
    pub async fn get_last_session_activities(&self) -> Result<Vec<Activity>> {
        let activities = self.activities.read().await;

        // Find last significant gap (> 30 minutes)
        let mut last_session = Vec::new();
        let mut last_time = Utc::now();

        for activity in activities.iter().rev() {
            let time_diff = last_time.signed_duration_since(activity.timestamp);

            if time_diff.num_minutes() > 30 && !last_session.is_empty() {
                break;
            }

            last_session.push(activity.clone());
            last_time = activity.timestamp;
        }

        last_session.reverse();
        Ok(last_session)
    }

    /// Get activity patterns for a time range
    pub async fn get_patterns(&self, days: u32) -> Result<ActivityPatterns> {
        let activities = self.activities.read().await;
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);

        let recent: Vec<_> = activities
            .iter()
            .filter(|a| a.timestamp > cutoff)
            .cloned()
            .collect();

        Ok(self.analyze_patterns(&recent))
    }

    /// Get time-based activity distribution
    pub async fn get_hourly_distribution(&self) -> Result<HashMap<u32, usize>> {
        let activities = self.activities.read().await;
        let mut distribution = HashMap::new();

        for activity in activities.iter() {
            let hour = activity.timestamp.hour();
            *distribution.entry(hour).or_insert(0) += 1;
        }

        Ok(distribution)
    }

    // Private helper methods

    async fn update_indexes(&self, activity: &Activity) -> Result<()> {
        // Update date index
        let date = activity.timestamp.date_naive();
        self.by_date
            .write()
            .await
            .entry(date)
            .or_insert_with(Vec::new)
            .push(activity.id);

        // Update project index
        if let Some(ref project) = activity.project {
            self.by_project
                .write()
                .await
                .entry(project.clone())
                .or_insert_with(Vec::new)
                .push(activity.id);
        }

        // Update type index
        let type_key = match &activity.activity_type {
            ActivityType::FileEdited(_) => "file_edited",
            ActivityType::Search(_) => "search",
            ActivityType::DocumentAdded(_) => "document_added",
            ActivityType::TaskCompleted(_) => "task_completed",
            ActivityType::CommandExecuted(_) => "command_executed",
            ActivityType::ProjectSwitched(_) => "project_switched",
            ActivityType::ResultClicked { .. } => "result_clicked",
            ActivityType::ResultViewed { .. } => "result_viewed",
            ActivityType::ResultIgnored { .. } => "result_ignored",
        };

        self.by_type
            .write()
            .await
            .entry(type_key.to_string())
            .or_insert_with(Vec::new)
            .push(activity.id);

        Ok(())
    }

    async fn update_stats(&self, activity: &Activity) -> Result<()> {
        let mut stats = self.stats.write().await;

        stats.total_activities += 1;

        // Update today's count
        if activity.timestamp.date_naive() == Utc::now().date_naive() {
            stats.activities_today += 1;
        }

        // Track searches for common patterns
        if let ActivityType::Search(query) = &activity.activity_type {
            // Maintain frequency count of searches
            if !stats.common_searches.contains(query) && stats.common_searches.len() < 10 {
                stats.common_searches.push(query.clone());
            }
        }

        Ok(())
    }

    fn analyze_patterns(&self, activities: &[Activity]) -> ActivityPatterns {
        let mut patterns = ActivityPatterns::default();

        // Analyze time patterns
        let mut hourly_counts = HashMap::new();
        for activity in activities {
            let hour = activity.timestamp.hour();
            *hourly_counts.entry(hour).or_insert(0) += 1;
        }

        patterns.most_active_hour = hourly_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(hour, _)| *hour);

        // Analyze project patterns
        let mut project_counts = HashMap::new();
        for activity in activities {
            if let Some(ref project) = activity.project {
                *project_counts.entry(project.clone()).or_insert(0) += 1;
            }
        }

        patterns.most_active_project = project_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(project, _)| project.clone());

        // Analyze activity sequences
        patterns.common_sequences = self.find_common_sequences(activities);

        patterns
    }

    fn find_common_sequences(&self, activities: &[Activity]) -> Vec<ActivitySequence> {
        let mut sequences = Vec::new();

        // Simple bigram analysis
        let mut bigrams = HashMap::new();

        for window in activities.windows(2) {
            if let [first, second] = window {
                let key = format!(
                    "{:?} -> {:?}",
                    self.activity_type_name(&first.activity_type),
                    self.activity_type_name(&second.activity_type)
                );
                *bigrams.entry(key).or_insert(0) += 1;
            }
        }

        // Convert to sequences
        for (sequence, count) in bigrams.iter() {
            if *count > 2 {
                // Only include patterns that occur more than twice
                sequences.push(ActivitySequence {
                    pattern: sequence.clone(),
                    frequency: *count,
                    confidence: (*count as f32) / (activities.len() as f32),
                });
            }
        }

        sequences.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        sequences.truncate(5); // Keep top 5

        sequences
    }

    fn activity_type_name(&self, activity_type: &ActivityType) -> &str {
        match activity_type {
            ActivityType::FileEdited(_) => "FileEdit",
            ActivityType::Search(_) => "Search",
            ActivityType::DocumentAdded(_) => "DocAdd",
            ActivityType::TaskCompleted(_) => "TaskDone",
            ActivityType::CommandExecuted(_) => "Command",
            ActivityType::ProjectSwitched(_) => "ProjectSwitch",
            ActivityType::ResultClicked { .. } => "Click",
            ActivityType::ResultViewed { .. } => "View",
            ActivityType::ResultIgnored { .. } => "Ignore",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityPatterns {
    pub most_active_hour: Option<u32>,
    pub most_active_project: Option<String>,
    pub common_sequences: Vec<ActivitySequence>,
    pub peak_productivity_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivitySequence {
    pub pattern: String,
    pub frequency: usize,
    pub confidence: f32,
}
