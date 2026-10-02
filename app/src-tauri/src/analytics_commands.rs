//! Analytics and Metrics Collection System
//! Tracks real usage data with persistent storage across restarts.

use crate::rag_commands::RagState;
use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::State;

/// Persistent analytics record — saved to disk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentAnalytics {
    pub total_queries: u64,
    pub total_errors: u64,
    pub total_indexing_ops: u64,
    pub query_log: Vec<QueryEvent>,
    pub hourly_queries: HashMap<String, u32>,
    pub hourly_response_times: HashMap<String, Vec<f64>>,
    pub query_counts: HashMap<String, QueryAgg>,
}

impl Default for PersistentAnalytics {
    fn default() -> Self {
        Self {
            total_queries: 0,
            total_errors: 0,
            total_indexing_ops: 0,
            query_log: Vec::new(),
            hourly_queries: HashMap::new(),
            hourly_response_times: HashMap::new(),
            query_counts: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryEvent {
    pub query: String,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: f64,
    pub result_count: u32,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryAgg {
    pub count: u32,
    pub total_time_ms: f64,
    pub total_results: u64,
    pub last_used: DateTime<Utc>,
}

/// Analytics state held in memory, backed by persistent storage
#[derive(Debug, Clone)]
pub struct AnalyticsState {
    pub data: Arc<Mutex<PersistentAnalytics>>,
    pub app_start: Instant,
    pub app_start_utc: DateTime<Utc>,
    pub storage_path: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl Default for AnalyticsState {
    fn default() -> Self {
        Self {
            data: Arc::new(Mutex::new(PersistentAnalytics::default())),
            app_start: Instant::now(),
            app_start_utc: Utc::now(),
            storage_path: Arc::new(Mutex::new(None)),
        }
    }
}

impl AnalyticsState {
    pub fn load_or_default(path: &std::path::Path) -> Self {
        let data = if path.exists() {
            match std::fs::read_to_string(path) {
                Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
                Err(_) => PersistentAnalytics::default(),
            }
        } else {
            PersistentAnalytics::default()
        };

        Self {
            data: Arc::new(Mutex::new(data)),
            app_start: Instant::now(),
            app_start_utc: Utc::now(),
            storage_path: Arc::new(Mutex::new(Some(path.to_path_buf()))),
        }
    }

    fn save(&self) {
        let path_guard = self.storage_path.lock().ok();
        let path = path_guard.as_ref().and_then(|p| p.as_ref());
        if let Some(path) = path {
            if let Ok(data) = self.data.lock() {
                if let Ok(json) = serde_json::to_string_pretty(&*data) {
                    let tmp = path.with_extension("json.tmp");
                    if std::fs::write(&tmp, &json).is_ok() {
                        let _ = std::fs::rename(&tmp, path);
                    }
                }
            }
        }
    }

    /// Compute the size of the app data directory in MB
    fn storage_size_mb(&self) -> f64 {
        let path_guard = self.storage_path.lock().ok();
        let path = path_guard.as_ref().and_then(|p| p.as_ref());
        if let Some(path) = path {
            if let Some(parent) = path.parent() {
                return dir_size_bytes(parent) as f64 / (1024.0 * 1024.0);
            }
        }
        0.0
    }
}

fn dir_size_bytes(path: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                } else if meta.is_dir() {
                    total += dir_size_bytes(&entry.path());
                }
            }
        }
    }
    total
}

// ─── Dashboard response types ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardData {
    pub overview: OverviewStats,
    pub performance_chart: Vec<TimeSeriesPoint>,
    pub usage_chart: Vec<TimeSeriesPoint>,
    pub top_queries: Vec<QueryStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewStats {
    pub total_documents: u64,
    pub total_chunks: u64,
    pub total_queries: u64,
    pub total_errors: u64,
    pub avg_response_time_ms: f64,
    pub storage_used_mb: f64,
    pub uptime_seconds: f64,
    pub error_rate_percent: f64,
    pub queries_last_hour: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesPoint {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryStat {
    pub query: String,
    pub count: u32,
    pub avg_time_ms: f64,
    pub avg_results: f64,
    pub last_used: DateTime<Utc>,
}

// ─── Commands ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_dashboard_data(
    analytics: State<'_, AnalyticsState>,
    rag_state: State<'_, RagState>,
) -> Result<DashboardData, String> {
    let data = analytics.data.lock().map_err(|e| e.to_string())?.clone();

    let rag_guard = rag_state.rag.read().await;
    let stats = rag_guard.get_statistics().await.unwrap_or_default();
    let total_chunks: u64 = stats
        .get("total_chunks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let total_documents: u64 = stats
        .get("total_documents")
        .and_then(|s| s.parse().ok())
        .unwrap_or(total_chunks);

    let uptime_secs = analytics.app_start.elapsed().as_secs_f64();

    let error_rate = if data.total_queries > 0 {
        (data.total_errors as f64 / data.total_queries as f64) * 100.0
    } else {
        0.0
    };

    // Compute avg response time from recent successful queries
    let recent_successful: Vec<&QueryEvent> = data
        .query_log
        .iter()
        .rev()
        .filter(|q| q.success)
        .take(100)
        .collect();
    let avg_response_time_ms = if !recent_successful.is_empty() {
        recent_successful.iter().map(|q| q.duration_ms).sum::<f64>()
            / recent_successful.len() as f64
    } else {
        0.0
    };

    // Queries in the last hour
    let now = Utc::now();
    let hour_key = format!("{}", now.format("%Y-%m-%d-%H"));
    let queries_last_hour = data.hourly_queries.get(&hour_key).copied().unwrap_or(0);

    let storage_used_mb = analytics.storage_size_mb();

    let overview = OverviewStats {
        total_documents,
        total_chunks,
        total_queries: data.total_queries,
        total_errors: data.total_errors,
        avg_response_time_ms,
        storage_used_mb,
        uptime_seconds: uptime_secs,
        error_rate_percent: error_rate,
        queries_last_hour,
    };

    // Build real time series from hourly data (last 24h)
    let usage_chart = build_hourly_chart(&data.hourly_queries, now);
    let performance_chart = build_performance_chart(&data.hourly_response_times, now);

    // Top queries from real aggregated data
    let mut top_queries: Vec<QueryStat> = data
        .query_counts
        .iter()
        .map(|(query, agg)| QueryStat {
            query: query.clone(),
            count: agg.count,
            avg_time_ms: if agg.count > 0 {
                agg.total_time_ms / agg.count as f64
            } else {
                0.0
            },
            avg_results: if agg.count > 0 {
                agg.total_results as f64 / agg.count as f64
            } else {
                0.0
            },
            last_used: agg.last_used,
        })
        .collect();
    top_queries.sort_by(|a, b| b.count.cmp(&a.count));
    top_queries.truncate(10);

    Ok(DashboardData {
        overview,
        performance_chart,
        usage_chart,
        top_queries,
    })
}

#[tauri::command]
pub async fn track_query(
    analytics: State<'_, AnalyticsState>,
    query: String,
    duration_ms: f64,
    result_count: u32,
) -> Result<(), String> {
    let now = Utc::now();
    let hour_key = format!("{}", now.format("%Y-%m-%d-%H"));
    let query_lower = query.to_lowercase().trim().to_string();

    {
        let mut data = analytics.data.lock().map_err(|e| e.to_string())?;

        data.total_queries += 1;

        // Ring buffer: keep last 2000 events
        if data.query_log.len() > 2000 {
            data.query_log.drain(0..500);
        }
        data.query_log.push(QueryEvent {
            query: query.clone(),
            timestamp: now,
            duration_ms,
            result_count,
            success: true,
        });

        // Hourly buckets
        *data.hourly_queries.entry(hour_key.clone()).or_insert(0) += 1;
        data.hourly_response_times
            .entry(hour_key)
            .or_default()
            .push(duration_ms);

        // Query aggregation
        let agg = data.query_counts.entry(query_lower).or_insert(QueryAgg {
            count: 0,
            total_time_ms: 0.0,
            total_results: 0,
            last_used: now,
        });
        agg.count += 1;
        agg.total_time_ms += duration_ms;
        agg.total_results += result_count as u64;
        agg.last_used = now;

        // Prune old hourly data (keep 72h)
        let cutoff = (now - Duration::hours(72))
            .format("%Y-%m-%d-%H")
            .to_string();
        data.hourly_queries
            .retain(|k, _| k.as_str() >= cutoff.as_str());
        data.hourly_response_times
            .retain(|k, _| k.as_str() >= cutoff.as_str());
    }

    analytics.save();
    Ok(())
}

#[tauri::command]
pub async fn track_query_error(
    analytics: State<'_, AnalyticsState>,
    query: String,
    _error_msg: String,
) -> Result<(), String> {
    let now = Utc::now();
    {
        let mut data = analytics.data.lock().map_err(|e| e.to_string())?;
        data.total_queries += 1;
        data.total_errors += 1;
        if data.query_log.len() > 2000 {
            data.query_log.drain(0..500);
        }
        data.query_log.push(QueryEvent {
            query,
            timestamp: now,
            duration_ms: 0.0,
            result_count: 0,
            success: false,
        });
    }
    analytics.save();
    Ok(())
}

#[tauri::command]
pub async fn track_indexing(
    analytics: State<'_, AnalyticsState>,
    _doc_type: String,
    _duration_ms: f64,
    doc_count: u32,
) -> Result<(), String> {
    {
        let mut data = analytics.data.lock().map_err(|e| e.to_string())?;
        data.total_indexing_ops += doc_count as u64;
    }
    analytics.save();
    Ok(())
}

#[tauri::command]
pub async fn get_performance_metrics(
    analytics: State<'_, AnalyticsState>,
) -> Result<serde_json::Value, String> {
    let data = analytics.data.lock().map_err(|e| e.to_string())?;
    let recent: Vec<&QueryEvent> = data
        .query_log
        .iter()
        .rev()
        .filter(|q| q.success)
        .take(50)
        .collect();
    let avg_ms = if !recent.is_empty() {
        recent.iter().map(|q| q.duration_ms).sum::<f64>() / recent.len() as f64
    } else {
        0.0
    };
    Ok(serde_json::json!({
        "avg_query_time_ms": avg_ms,
        "total_queries": data.total_queries,
        "total_errors": data.total_errors,
        "storage_size_mb": analytics.storage_size_mb(),
    }))
}

#[tauri::command]
pub async fn get_usage_metrics(
    analytics: State<'_, AnalyticsState>,
) -> Result<serde_json::Value, String> {
    let data = analytics.data.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(&*data).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_quality_metrics(
    analytics: State<'_, AnalyticsState>,
) -> Result<serde_json::Value, String> {
    let data = analytics.data.lock().map_err(|e| e.to_string())?;
    let recent: Vec<&QueryEvent> = data.query_log.iter().rev().take(100).collect();
    let with_results = recent.iter().filter(|q| q.result_count > 0).count();
    let hit_rate = if !recent.is_empty() {
        with_results as f64 / recent.len() as f64
    } else {
        1.0
    };
    Ok(serde_json::json!({
        "hit_rate": hit_rate,
        "total_queries": data.total_queries,
        "total_errors": data.total_errors,
    }))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn build_hourly_chart(hourly: &HashMap<String, u32>, now: DateTime<Utc>) -> Vec<TimeSeriesPoint> {
    let mut points = Vec::with_capacity(24);
    for i in (0..24).rev() {
        let ts = now - Duration::hours(i);
        let key = format!("{}", ts.format("%Y-%m-%d-%H"));
        let value = hourly.get(&key).copied().unwrap_or(0) as f64;
        points.push(TimeSeriesPoint {
            timestamp: ts,
            value,
            label: Some(format!("{:02}:00", ts.hour())),
        });
    }
    points
}

fn build_performance_chart(
    hourly_times: &HashMap<String, Vec<f64>>,
    now: DateTime<Utc>,
) -> Vec<TimeSeriesPoint> {
    let mut points = Vec::with_capacity(24);
    for i in (0..24).rev() {
        let ts = now - Duration::hours(i);
        let key = format!("{}", ts.format("%Y-%m-%d-%H"));
        let value = hourly_times
            .get(&key)
            .map(|times| {
                if times.is_empty() {
                    0.0
                } else {
                    times.iter().sum::<f64>() / times.len() as f64
                }
            })
            .unwrap_or(0.0);
        points.push(TimeSeriesPoint {
            timestamp: ts,
            value,
            label: Some(format!("{:02}:00", ts.hour())),
        });
    }
    points
}
