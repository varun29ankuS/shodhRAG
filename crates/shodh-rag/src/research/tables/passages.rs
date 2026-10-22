//! Where a paper's passages come from: the hybrid search restricted to the paper (its
//! stored `source`), the paper's first chunk, and for `number` columns the accepted
//! Result statements already read from its tables, whose cell boxes become the highlight.
//!
//! Reading is all this module does. It takes the engine's read lock and never changes
//! the index, so it cannot reach the library's removal observer (the tables forgetting a
//! paper) however a search or a lookup ends.

use std::collections::HashMap;

use serde_json::json;

use super::Passage;
use crate::rag_engine::RAGEngine;
use crate::research::path_key;
use crate::research::results::{ResultRecord, ResultStatus};
use crate::types::MetadataFilter;

/// Search over one paper's chunks. Implemented by the RAG engine; tests use doubles.
#[async_trait::async_trait]
pub trait PaperIndex: Send + Sync {
    /// The `k` passages of `source` that best match `query`, best first.
    async fn search_paper(
        &self,
        source: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<Passage>, String>;
    /// The first chunk of `source`, or `None` when the paper has no chunks.
    async fn first_passage(&self, source: &str) -> Result<Option<Passage>, String>;
}

/// A passage from a chunk's id, text and stored metadata (`page`, `bboxes`).
pub fn passage_from(chunk_id: String, text: String, metadata: &HashMap<String, String>) -> Passage {
    Passage {
        chunk_id,
        text,
        page: metadata.get("page").and_then(|p| p.trim().parse().ok()),
        bboxes: metadata
            .get("bboxes")
            .filter(|b| !b.trim().is_empty())
            .cloned(),
    }
}

/// Chunk ids in one spelling (search results carry parsed UUIDs, listed chunks the
/// stored string), so the first chunk and a search hit of it are one passage.
fn chunk_id(id: &str) -> String {
    uuid::Uuid::parse_str(id)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| id.to_string())
}

#[async_trait::async_trait]
impl PaperIndex for tokio::sync::RwLock<RAGEngine> {
    async fn search_paper(
        &self,
        source: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<Passage>, String> {
        let filter = MetadataFilter {
            source_path: Some(path_key(source)),
            ..Default::default()
        };
        let engine = self.read().await;
        let hits = engine
            .search_comprehensive(query, k, Some(filter))
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(hits
            .into_iter()
            .map(|hit| passage_from(chunk_id(&hit.id.to_string()), hit.snippet, &hit.metadata))
            .collect())
    }

    async fn first_passage(&self, source: &str) -> Result<Option<Passage>, String> {
        let engine = self.read().await;
        let hit = engine
            .first_chunk(source)
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(hit.map(|hit| {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&hit.metadata_json).unwrap_or_default();
            passage_from(chunk_id(&hit.id), hit.text, &metadata)
        }))
    }
}

/// The accepted Results of a paper as passages: one line per Result
/// ("BM25 on MS MARCO: Recall@10 = 0.355 (k1 = 0.9). Table: …"), its page and its cell box.
pub fn result_passages(records: &[ResultRecord]) -> Vec<Passage> {
    records
        .iter()
        .filter(|r| r.status == ResultStatus::Accepted)
        .map(|r| {
            let mut text = format!(
                "{} on {}: {} = {}",
                r.method, r.dataset, r.metric, r.value_text
            );
            if let Some(setting) = r.setting.as_deref().filter(|s| !s.trim().is_empty()) {
                text.push_str(&format!(" ({setting})"));
            }
            if let Some(caption) = r.table_caption.as_deref().filter(|c| !c.trim().is_empty()) {
                text.push_str(&format!(". Table: {caption}"));
            }
            Passage {
                chunk_id: format!("result:{}", r.id),
                text,
                page: Some(r.page),
                bboxes: r.region.map(|g| {
                    json!([{ "page": g.page, "x0": g.x0, "y0": g.y0, "x1": g.x1, "y1": g.y1 }])
                        .to_string()
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use shodh_ontology::ExtractorKind;

    use super::*;
    use crate::research::results::Region;
    use crate::statements::Scope;

    fn record(id: &str, status: ResultStatus) -> ResultRecord {
        ResultRecord {
            id: id.to_string(),
            method: "BM25".to_string(),
            dataset: "MS MARCO".to_string(),
            metric: "Recall@10".to_string(),
            method_id: "method:bm25".to_string(),
            dataset_id: "dataset:ms-marco".to_string(),
            metric_id: "metric:recall-10".to_string(),
            value: "0.355".to_string(),
            value_text: "0.355".to_string(),
            unit: None,
            spread: None,
            setting: Some("k1 = 0.9".to_string()),
            file_path: "/p/a.pdf".to_string(),
            file_name: "a.pdf".to_string(),
            page: 7,
            region: Some(Region {
                page: 7,
                x0: 10.0,
                y0: 20.0,
                x1: 30.0,
                y1: 40.0,
            }),
            table_caption: Some("Table 2: Retrieval results.".to_string()),
            extractor: ExtractorKind::User,
            confidence: 1.0,
            status,
            scope: Scope::Global,
            extracted_at: Utc::now(),
        }
    }

    #[test]
    fn accepted_results_become_passages_with_their_cell_box() {
        let passages = result_passages(&[
            record("r1", ResultStatus::Accepted),
            record("r2", ResultStatus::Review),
        ]);
        assert_eq!(passages.len(), 1);
        let p = &passages[0];
        assert_eq!(p.chunk_id, "result:r1");
        assert_eq!(
            p.text,
            "BM25 on MS MARCO: Recall@10 = 0.355 (k1 = 0.9). Table: Table 2: Retrieval results."
        );
        assert_eq!(p.page, Some(7));
        let boxes: serde_json::Value = serde_json::from_str(p.bboxes.as_deref().unwrap()).unwrap();
        assert_eq!(boxes[0]["page"], 7);
        assert_eq!(boxes[0]["x1"], 30.0);
    }

    #[test]
    fn chunk_metadata_gives_the_page_and_boxes() {
        let mut metadata = HashMap::new();
        metadata.insert("page".to_string(), "4".to_string());
        metadata.insert(
            "bboxes".to_string(),
            r#"[{"page":4,"x0":1.0,"y0":2.0,"x1":3.0,"y1":4.0}]"#.to_string(),
        );
        let p = passage_from("id".into(), "text".into(), &metadata);
        assert_eq!(p.page, Some(4));
        assert!(p.bboxes.is_some());
        let bare = passage_from("id".into(), "text".into(), &HashMap::new());
        assert_eq!((bare.page, bare.bboxes), (None, None));
        let id = uuid::Uuid::new_v4();
        assert_eq!(chunk_id(&id.to_string().to_uppercase()), id.to_string());
    }
}
