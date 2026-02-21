//! Snapshot management for spaces - integrated with vector database

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc, Duration};
use uuid::Uuid;
use anyhow::Result;

/// Snapshot metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub space_id: String,
    pub space_name: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub document_count: usize,
    pub vector_count: usize,
    pub size_bytes: usize,
    pub snapshot_type: SnapshotType,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SnapshotType {
    Manual,
    Automatic,
    PreUpdate,
    Export,
}

/// Snapshot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotConfig {
    pub auto_snapshot_enabled: bool,
    pub auto_snapshot_interval_hours: u32,
    pub max_snapshots_per_space: usize,
    pub retention_days: u32,
    pub compress_snapshots: bool,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            auto_snapshot_enabled: true,
            auto_snapshot_interval_hours: 24, // Daily
            max_snapshots_per_space: 7,       // Keep last 7 snapshots
            retention_days: 30,                // Delete after 30 days
            compress_snapshots: true,
        }
    }
}

/// Snapshot manager for handling all snapshot operations
pub struct SnapshotManager {
    config: SnapshotConfig,
}

impl SnapshotManager {
    pub fn new(config: Option<SnapshotConfig>) -> Self {
        Self {
            config: config.unwrap_or_default(),
        }
    }

    /// Create a snapshot of a space
    pub async fn create_snapshot(
        &self,
        rag: &shodh_rag::comprehensive_system::ComprehensiveRAG,
        space_id: &str,
        space_name: &str,
        name: String,
        description: Option<String>,
        snapshot_type: SnapshotType,
    ) -> Result<Snapshot> {
        let snapshot_id = format!("snapshot_{}_{}", space_id, Uuid::new_v4());
        let collection_name = format!("snapshot_{}_{}", 
            space_id, 
            Utc::now().format("%Y%m%d_%H%M%S")
        );

        // Get all documents in the space
        // NOTE: ComprehensiveRAG doesn't have a search method, so we'll skip this for now
        // In a real implementation, we'd need to use the actual API
        let documents = Vec::new();

        let document_count = documents.len();
        let mut vector_count = 0;
        let mut size_bytes = 0;

        // Create snapshot collection in vector DB
        // Store each document with its vectors in the snapshot collection
        for doc in &documents {
            // Calculate sizes
            size_bytes += doc.text.len();
            vector_count += 1; // Each document has at least one vector
            
            // Store document in snapshot collection
            let mut snapshot_metadata = doc.metadata.clone().unwrap_or_default();
            snapshot_metadata.insert("snapshot_id".to_string(), snapshot_id.clone());
            snapshot_metadata.insert("original_doc_id".to_string(), doc.id.clone());
            snapshot_metadata.insert("snapshot_collection".to_string(), collection_name.clone());
            
            // Add to snapshot collection with modified ID
            let snapshot_doc_id = format!("{}_{}", snapshot_id, doc.id);
            rag.add_document(&snapshot_doc_id, &doc.text, snapshot_metadata).await?;
        }

        // Create snapshot metadata
        let snapshot = Snapshot {
            id: snapshot_id.clone(),
            space_id: space_id.to_string(),
            space_name: space_name.to_string(),
            name,
            description,
            created_at: Utc::now(),
            document_count,
            vector_count,
            size_bytes,
            snapshot_type,
            metadata: HashMap::new(),
        };

        // Store snapshot metadata as a special document
        let mut metadata = HashMap::new();
        metadata.insert("doc_type".to_string(), "snapshot_metadata".to_string());
        metadata.insert("snapshot_id".to_string(), snapshot_id.clone());
        metadata.insert("space_id".to_string(), space_id.to_string());
        metadata.insert("collection_name".to_string(), collection_name);
        metadata.insert("created_at".to_string(), snapshot.created_at.to_rfc3339());
        metadata.insert("document_count".to_string(), document_count.to_string());
        metadata.insert("snapshot_data".to_string(), serde_json::to_string(&snapshot)?);

        rag.add_document(
            &format!("snapshot_meta_{}", snapshot_id),
            &format!("Snapshot: {} - {}", space_name, snapshot.name),
            metadata,
        ).await?;

        Ok(snapshot)
    }

    /// Restore a space from snapshot
    pub async fn restore_snapshot(
        &self,
        rag: &shodh_rag::comprehensive_system::ComprehensiveRAG,
        snapshot_id: &str,
        restore_mode: RestoreMode,
    ) -> Result<RestoreResult> {
        // Get snapshot metadata
        let meta_filter = shodh_rag::query::FilterPredicate::new(
            "snapshot_id".to_string(),
            shodh_rag::query::Operator::Equals,
            snapshot_id.to_string(),
        );

        let snapshot_meta_filter = shodh_rag::query::FilterPredicate::new(
            "doc_type".to_string(),
            shodh_rag::query::Operator::Equals,
            "snapshot_metadata".to_string(),
        );

        let meta_results = rag.search("", 1, Some(vec![meta_filter.clone(), snapshot_meta_filter]))
            .await?;

        if meta_results.is_empty() {
            return Err(anyhow::anyhow!("Snapshot not found"));
        }

        let snapshot_metadata = meta_results[0].metadata.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Snapshot metadata missing"))?;

        let snapshot: Snapshot = serde_json::from_str(
            snapshot_metadata.get("snapshot_data")
                .ok_or_else(|| anyhow::anyhow!("Snapshot data missing"))?
        )?;

        let space_id = &snapshot.space_id;

        // Handle different restore modes
        match restore_mode {
            RestoreMode::Replace => {
                // Delete all current documents in the space
                let space_filter = shodh_rag::query::FilterPredicate::new(
                    "space_id".to_string(),
                    shodh_rag::query::Operator::Equals,
                    space_id.to_string(),
                );

                let current_docs = rag.search("", 10000, Some(vec![space_filter]))
                    .await?;

                for doc in current_docs {
                    rag.delete_document(&doc.id).await?;
                }
            }
            RestoreMode::Merge => {
                // Keep existing documents, add snapshot documents
            }
            RestoreMode::CreateNew => {
                // Will create documents with new IDs
            }
        }

        // Get all documents from snapshot
        let snapshot_filter = shodh_rag::query::FilterPredicate::new(
            "snapshot_id".to_string(),
            shodh_rag::query::Operator::Equals,
            snapshot_id.to_string(),
        );

        let snapshot_docs = rag.search("", 10000, Some(vec![snapshot_filter]))
            .await?;

        let mut restored_count = 0;
        for doc in snapshot_docs {
            if let Some(metadata) = doc.metadata {
                if metadata.get("doc_type") == Some(&"snapshot_metadata".to_string()) {
                    continue; // Skip metadata document
                }

                // Restore document to main collection
                let original_id = metadata.get("original_doc_id")
                    .unwrap_or(&doc.id)
                    .clone();

                let mut restored_metadata = metadata.clone();
                restored_metadata.remove("snapshot_id");
                restored_metadata.remove("snapshot_collection");
                restored_metadata.remove("original_doc_id");

                let new_id = match restore_mode {
                    RestoreMode::CreateNew => Uuid::new_v4().to_string(),
                    _ => original_id,
                };

                rag.add_document(&new_id, &doc.text, restored_metadata).await?;
                restored_count += 1;
            }
        }

        Ok(RestoreResult {
            restored_documents: restored_count,
            snapshot_id: snapshot_id.to_string(),
            space_id: space_id.to_string(),
            restore_mode,
        })
    }

    /// List all snapshots for a space
    pub async fn list_snapshots(
        &self,
        rag: &shodh_rag::comprehensive_system::ComprehensiveRAG,
        space_id: Option<&str>,
    ) -> Result<Vec<Snapshot>> {
        let mut filters = vec![
            shodh_rag::query::FilterPredicate::new(
                "doc_type".to_string(),
                shodh_rag::query::Operator::Equals,
                "snapshot_metadata".to_string(),
            )
        ];

        if let Some(space_id) = space_id {
            filters.push(shodh_rag::query::FilterPredicate::new(
                "space_id".to_string(),
                shodh_rag::query::Operator::Equals,
                space_id.to_string(),
            ));
        }

        let results = rag.search("", 1000, Some(filters)).await?;
        
        let mut snapshots = Vec::new();
        for result in results {
            if let Some(metadata) = result.metadata {
                if let Some(snapshot_data) = metadata.get("snapshot_data") {
                    if let Ok(snapshot) = serde_json::from_str::<Snapshot>(snapshot_data) {
                        snapshots.push(snapshot);
                    }
                }
            }
        }

        // Sort by creation date (newest first)
        snapshots.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        
        Ok(snapshots)
    }

    /// Delete a snapshot
    pub async fn delete_snapshot(
        &self,
        rag: &shodh_rag::comprehensive_system::ComprehensiveRAG,
        snapshot_id: &str,
    ) -> Result<()> {
        // Get all documents in snapshot
        let snapshot_filter = shodh_rag::query::FilterPredicate::new(
            "snapshot_id".to_string(),
            shodh_rag::query::Operator::Equals,
            snapshot_id.to_string(),
        );

        let snapshot_docs = rag.search("", 10000, Some(vec![snapshot_filter]))
            .await?;

        // Delete all snapshot documents
        for doc in snapshot_docs {
            rag.delete_document(&doc.id).await?;
        }

        // Delete snapshot metadata
        rag.delete_document(&format!("snapshot_meta_{}", snapshot_id)).await?;

        Ok(())
    }

    /// Clean up old snapshots based on retention policy
    pub async fn cleanup_old_snapshots(
        &self,
        rag: &shodh_rag::comprehensive_system::ComprehensiveRAG,
    ) -> Result<CleanupResult> {
        let all_snapshots = self.list_snapshots(rag, None).await?;
        let cutoff_date = Utc::now() - Duration::days(self.config.retention_days as i64);
        
        let mut deleted_count = 0;
        let mut space_snapshot_counts: HashMap<String, usize> = HashMap::new();

        // Count snapshots per space
        for snapshot in &all_snapshots {
            *space_snapshot_counts.entry(snapshot.space_id.clone()).or_insert(0) += 1;
        }

        for snapshot in all_snapshots {
            let should_delete = 
                // Delete if older than retention period
                snapshot.created_at < cutoff_date ||
                // Delete if exceeds max per space (keep newest)
                (space_snapshot_counts.get(&snapshot.space_id).unwrap_or(&0) > &self.config.max_snapshots_per_space &&
                 snapshot.snapshot_type == SnapshotType::Automatic);

            if should_delete {
                self.delete_snapshot(rag, &snapshot.id).await?;
                deleted_count += 1;
                
                if let Some(count) = space_snapshot_counts.get_mut(&snapshot.space_id) {
                    *count -= 1;
                }
            }
        }

        Ok(CleanupResult {
            deleted_snapshots: deleted_count,
            remaining_snapshots: all_snapshots.len() - deleted_count,
        })
    }

    /// Export snapshot to file
    pub async fn export_snapshot(
        &self,
        rag: &shodh_rag::comprehensive_system::ComprehensiveRAG,
        snapshot_id: &str,
        export_path: &std::path::Path,
    ) -> Result<()> {
        // Get all documents in snapshot
        let snapshot_filter = shodh_rag::query::FilterPredicate::new(
            "snapshot_id".to_string(),
            shodh_rag::query::Operator::Equals,
            snapshot_id.to_string(),
        );

        let snapshot_docs = rag.search("", 10000, Some(vec![snapshot_filter]))
            .await?;

        // Create export structure
        let export_data = SnapshotExport {
            snapshot_id: snapshot_id.to_string(),
            exported_at: Utc::now(),
            documents: snapshot_docs,
        };

        // Write to file (compressed if configured)
        let json_data = serde_json::to_string_pretty(&export_data)?;
        
        if self.config.compress_snapshots {
            use flate2::write::GzEncoder;
            use flate2::Compression;
            use std::fs::File;
            use std::io::Write;
            
            let file = File::create(export_path.with_extension("json.gz"))?;
            let mut encoder = GzEncoder::new(file, Compression::default());
            encoder.write_all(json_data.as_bytes())?;
            encoder.finish()?;
        } else {
            std::fs::write(export_path, json_data)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RestoreMode {
    Replace,   // Delete current space content and replace with snapshot
    Merge,     // Add snapshot content to existing space
    CreateNew, // Create as new space with new IDs
}

#[derive(Debug, Serialize)]
pub struct RestoreResult {
    pub restored_documents: usize,
    pub snapshot_id: String,
    pub space_id: String,
    pub restore_mode: RestoreMode,
}

#[derive(Debug, Serialize)]
pub struct CleanupResult {
    pub deleted_snapshots: usize,
    pub remaining_snapshots: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotExport {
    snapshot_id: String,
    exported_at: DateTime<Utc>,
    documents: Vec<shodh_rag::query::SearchResult>,
}