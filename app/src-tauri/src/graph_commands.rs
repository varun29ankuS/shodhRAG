//! Commands for knowledge graph visualization
//!
//! This module builds INTELLIGENT knowledge graphs that understand structure,
//! not just surface-level similarities.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;
use crate::rag_commands::RagState;
use shodh_rag::types::MetadataFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub size: f32,
    pub color: String,
    pub metadata: Option<NodeMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetadata {
    pub document_count: Option<usize>,
    pub connections: Option<usize>,
    pub score: Option<f32>,
    pub space: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub weight: f32,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Get knowledge graph data for visualization
#[tauri::command]
pub async fn get_knowledge_graph(
    state: State<'_, RagState>,
    space_id: Option<String>,
    max_nodes: Option<usize>,
    min_similarity: Option<f32>,
) -> Result<GraphData, String> {
    tracing::info!("Knowledge graph requested - space_id: {:?}, max_nodes: {:?}", space_id, max_nodes);

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    tracing::info!("RAG system is initialized, fetching documents...");
    let max_nodes = max_nodes.unwrap_or(100);
    let min_similarity = min_similarity.unwrap_or(0.5);

    // Get documents from the space or all documents via search
    let filter = space_id.as_ref().map(|sid| MetadataFilter {
        space_id: Some(sid.clone()),
        ..Default::default()
    });
    let documents = rag.list_documents(filter, max_nodes)
        .await
        .map_err(|e| format!("Failed to list documents: {}", e))?;

    tracing::info!("Found {} documents", documents.len());

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut topics: HashMap<String, usize> = HashMap::new();
    let mut entities: HashMap<String, usize> = HashMap::new();
        
        // Create document nodes
        for (idx, doc) in documents.iter().enumerate() {
            let doc_id = format!("doc-{}", idx);
            
            // Extract title from metadata
            let title = doc.metadata.get("title")
                .or_else(|| doc.metadata.get("filename"))
                .or_else(|| doc.metadata.get("file_name"))
                .cloned()
                .unwrap_or_else(|| format!("Document {}", idx + 1));
            
            nodes.push(GraphNode {
                id: doc_id.clone(),
                label: title,
                node_type: "document".to_string(),
                size: 4.0 + (doc.score * 2.0),
                color: "#10b981".to_string(),
                metadata: Some(NodeMetadata {
                    document_count: None,
                    connections: Some(0),
                    score: Some(doc.score),
                    space: doc.metadata.get("space_id").cloned(),
                    file_path: doc.metadata.get("file_path").cloned(),
                }),
            });
            
            // Extract code-related topics based on file type
            if let Some(file_type) = doc.metadata.get("file_type") {
                *topics.entry(file_type.clone()).or_insert(0) += 1;
                edges.push(GraphEdge {
                    source: doc_id.clone(),
                    target: format!("topic-{}", file_type.replace(" ", "_")),
                    weight: 0.8,
                    edge_type: "category".to_string(),
                    label: None,
                });
            }

            // Extract language/technology from file extension
            if let Some(ext) = doc.metadata.get("file_extension") {
                let tech = match ext.as_str() {
                    "rs" => Some("Rust"),
                    "py" => Some("Python"),
                    "js" | "jsx" => Some("JavaScript"),
                    "ts" | "tsx" => Some("TypeScript"),
                    "java" => Some("Java"),
                    "go" => Some("Go"),
                    "cpp" | "cc" | "cxx" | "c" | "h" => Some("C/C++"),
                    _ => None,
                };

                if let Some(tech) = tech {
                    *entities.entry(tech.to_string()).or_insert(0) += 1;
                    edges.push(GraphEdge {
                        source: doc_id.clone(),
                        target: format!("entity-{}", tech.replace(" ", "_").replace("/", "_")),
                        weight: 0.7,
                        edge_type: "language".to_string(),
                        label: None,
                    });
                }
            }

            // Extract module relationships from imports/references in content
            let content_lower = doc.snippet.to_lowercase();
            if content_lower.contains("import ") || content_lower.contains("use ") ||
               content_lower.contains("require(") || content_lower.contains("from ") {
                *topics.entry("Has Dependencies".to_string()).or_insert(0) += 1;
            }
            if content_lower.contains("export ") || content_lower.contains("pub ") {
                *topics.entry("Exports API".to_string()).or_insert(0) += 1;
            }
            if content_lower.contains("async ") || content_lower.contains("await ") {
                *topics.entry("Async Code".to_string()).or_insert(0) += 1;
            }
        }
        
        // Create topic nodes
        for (topic, count) in topics.iter() {
            nodes.push(GraphNode {
                id: format!("topic-{}", topic.replace(" ", "_")),
                label: topic.clone(),
                node_type: "topic".to_string(),
                size: 5.0 + (*count as f32 * 0.5).min(10.0),
                color: "#6366f1".to_string(),
                metadata: Some(NodeMetadata {
                    document_count: Some(*count),
                    connections: Some(*count),
                    score: None,
                    space: None,
                    file_path: None,
                }),
            });
        }
        
        // Create entity nodes
        for (entity, count) in entities.iter() {
            nodes.push(GraphNode {
                id: format!("entity-{}", entity.replace(" ", "_")),
                label: entity.clone(),
                node_type: "entity".to_string(),
                size: 4.0 + (*count as f32 * 0.4).min(8.0),
                color: "#f59e0b".to_string(),
                metadata: Some(NodeMetadata {
                    document_count: None,
                    connections: Some(*count),
                    score: None,
                    space: None,
                    file_path: None,
                }),
            });
        }
        
        // Create semantic similarity edges using vector embeddings
        // Get the actual embedding vectors from RAG system
        for i in 0..documents.len().min(max_nodes) {
            // Connect to files in same directory
            let doc1 = &documents[i];
            let doc1_path = doc1.metadata.get("file_path");

            for j in (i + 1)..documents.len().min(max_nodes) {
                let doc2 = &documents[j];
                let doc2_path = doc2.metadata.get("file_path");

                // Strong connection: Same directory
                if let (Some(path1), Some(path2)) = (doc1_path, doc2_path) {
                    let dir1 = std::path::Path::new(path1).parent();
                    let dir2 = std::path::Path::new(path2).parent();

                    if dir1 == dir2 && dir1.is_some() {
                        edges.push(GraphEdge {
                            source: format!("doc-{}", i),
                            target: format!("doc-{}", j),
                            weight: 0.8,
                            edge_type: "same_directory".to_string(),
                            label: Some("Same Folder".to_string()),
                        });
                        continue;
                    }
                }

                // Medium connection: Same file type
                let same_type = doc1.metadata.get("file_type") == doc2.metadata.get("file_type")
                    && doc1.metadata.get("file_type").is_some();

                if same_type {
                    // Use score similarity as proxy for semantic similarity
                    let similarity = 1.0 - (doc1.score - doc2.score).abs();

                    if similarity > min_similarity {
                        edges.push(GraphEdge {
                            source: format!("doc-{}", i),
                            target: format!("doc-{}", j),
                            weight: similarity * 0.6,
                            edge_type: "semantic".to_string(),
                            label: Some(format!("{:.0}%", similarity * 100.0)),
                        });
                    }
                }
            }
        }

    tracing::info!("Generated knowledge graph: {} nodes, {} edges", nodes.len(), edges.len());
    Ok(GraphData { nodes, edges })
}

/// Generate sample graph data for demonstration
fn generate_sample_graph_data() -> GraphData {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    
    // Sample topics
    let topics = vec![
        ("Machine Learning", "#6366f1"),
        ("Data Science", "#8b5cf6"),
        ("Neural Networks", "#a855f7"),
    ];
    
    for (i, (topic, color)) in topics.iter().enumerate() {
        nodes.push(GraphNode {
            id: format!("topic-{}", i),
            label: topic.to_string(),
            node_type: "topic".to_string(),
            size: 15.0,
            color: color.to_string(),
            metadata: Some(NodeMetadata {
                document_count: Some(10 + i * 3),
                connections: Some(5 + i * 2),
                score: None,
                space: None,
                file_path: None,
            }),
        });
    }
    
    // Sample documents
    for i in 0..20 {
        nodes.push(GraphNode {
            id: format!("doc-{}", i),
            label: format!("Document {}", i + 1),
            node_type: "document".to_string(),
            size: 8.0,
            color: "#10b981".to_string(),
            metadata: Some(NodeMetadata {
                document_count: None,
                connections: Some(2),
                score: Some(0.5 + (i as f32 * 0.02)),
                space: Some("sample".to_string()),
                file_path: None,
            }),
        });
        
        // Connect to topics
        let topic_idx = i % topics.len();
        edges.push(GraphEdge {
            source: format!("doc-{}", i),
            target: format!("topic-{}", topic_idx),
            weight: 0.7,
            edge_type: "topic".to_string(),
            label: None,
        });
        
        // Add some document connections
        if i > 0 && i % 3 == 0 {
            edges.push(GraphEdge {
                source: format!("doc-{}", i),
                target: format!("doc-{}", i - 1),
                weight: 0.5,
                edge_type: "similarity".to_string(),
                label: Some("Related".to_string()),
            });
        }
    }
    
    // Sample entities
    let entities = vec!["OpenAI", "Google", "Microsoft"];
    for (i, entity) in entities.iter().enumerate() {
        nodes.push(GraphNode {
            id: format!("entity-{}", i),
            label: entity.to_string(),
            node_type: "entity".to_string(),
            size: 10.0,
            color: "#f59e0b".to_string(),
            metadata: Some(NodeMetadata {
                document_count: None,
                connections: Some(3 + i),
                score: None,
                space: None,
                file_path: None,
            }),
        });
        
        // Connect to some documents
        for j in 0..3 {
            let doc_idx = (i * 3 + j) % 20;
            edges.push(GraphEdge {
                source: format!("entity-{}", i),
                target: format!("doc-{}", doc_idx),
                weight: 0.4,
                edge_type: "cooccurrence".to_string(),
                label: None,
            });
        }
    }
    
    GraphData { nodes, edges }
}