//! Where a paper's passages come from: the hybrid search restricted to the paper (its
//! stored `source`) and the paper's first chunk. Both read the paper under one spelling of
//! its path ([`path_key`]), so a paper that one finds the other finds too.
//!
//! A passage is always real text of the paper: a search hit is cut to the chunk that
//! matched (the neighbours the engine joins to a snippet are not part of it), so the
//! chunk id, page and boxes of a passage are those of the text it holds. The Results
//! already read from the paper's tables are not passages: a line assembled from table
//! cells ("BM25 on MS MARCO: Recall@10 = 0.355") is not verbatim text, and a quote from it
//! would pass the check without being in the paper. They only help the search of a
//! `number` column find the real table text ([`result_hint_terms`]).
//!
//! Reading is all this module does. It takes the engine's read lock and never changes
//! the index, so it cannot reach the library's removal observer (the tables forgetting a
//! paper) however a search or a lookup ends.

use std::collections::{HashMap, HashSet};

use super::Passage;
use crate::rag_engine::{RAGEngine, SnippetParts};
use crate::research::path_key;
use crate::research::results::ResultRecord;
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

/// The text of the chunk that matched. The engine widens the snippet of a hit with its
/// neighbouring chunks and records where the matched chunk lies in it; the chunk id, page
/// and boxes of the hit describe that chunk only, so the passage holds that text only.
/// A snippet that was not widened is returned as it is.
pub fn matched_text(snippet: String, metadata: &HashMap<String, String>) -> String {
    match SnippetParts::of(&snippet, metadata) {
        Some(parts) => {
            let (start, end) = parts.matched;
            snippet[start..end].to_string()
        }
        None => snippet,
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
            .map(|hit| {
                let text = matched_text(hit.snippet, &hit.metadata);
                passage_from(chunk_id(&hit.id.to_string()), text, &hit.metadata)
            })
            .collect())
    }

    async fn first_passage(&self, source: &str) -> Result<Option<Passage>, String> {
        let engine = self.read().await;
        let hit = engine
            .first_chunk(&path_key(source))
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(hit.map(|hit| {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&hit.metadata_json).unwrap_or_default();
            passage_from(chunk_id(&hit.id), hit.text, &metadata)
        }))
    }
}

/// Most Results whose words are added to the search of one `number` column.
pub const RESULT_HINTS_PER_COLUMN: usize = 3;

/// Words that say nothing about what a column asks.
const STOP_WORDS: [&str; 16] = [
    "the", "and", "what", "which", "how", "are", "for", "with", "from", "that", "this", "does",
    "paper", "reported", "report", "used",
];

fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// Extra search words for a `number` column: the method, dataset and metric of the (at
/// most `limit`) Results of the paper that share the most words with the column's question
/// and instructions. Results with nothing in common with them are left out.
pub fn result_hint_terms(
    records: &[ResultRecord],
    question: &str,
    instructions: &str,
    limit: usize,
) -> String {
    let wanted = words(&format!("{question} {instructions}"));
    let mut scored: Vec<(usize, &ResultRecord)> = records
        .iter()
        .map(|r| {
            let text = format!(
                "{} {} {} {} {}",
                r.method,
                r.dataset,
                r.metric,
                r.setting.as_deref().unwrap_or(""),
                r.table_caption.as_deref().unwrap_or("")
            );
            (words(&text).intersection(&wanted).count(), r)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    // Best first; equal scores keep the order the Results came in.
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, r)| format!("{} {} {}", r.method, r.dataset, r.metric))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use shodh_ontology::ExtractorKind;

    use super::*;
    use crate::rag_engine::{NeighborPart, SNIPPET_PARTS_KEY};
    use crate::research::results::{Region, ResultStatus};
    use crate::statements::Scope;

    fn record(id: &str, method: &str, dataset: &str, metric: &str) -> ResultRecord {
        ResultRecord {
            id: id.to_string(),
            method: method.to_string(),
            dataset: dataset.to_string(),
            metric: metric.to_string(),
            method_id: "method:x".to_string(),
            dataset_id: "dataset:x".to_string(),
            metric_id: "metric:x".to_string(),
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
            status: ResultStatus::Accepted,
            scope: Scope::Global,
            extracted_at: Utc::now(),
        }
    }

    #[test]
    fn results_only_hint_the_search_and_the_most_relevant_few_are_kept() {
        let records = vec![
            record("r1", "BM25", "MS MARCO", "Recall@10"),
            record("r2", "DPR", "Natural Questions", "Exact match"),
            record("r3", "ColBERT", "MS MARCO", "MRR@10"),
            record("r4", "SPLADE", "MS MARCO", "Recall@10"),
            record("r5", "TILDE", "TREC DL", "nDCG"),
        ];
        let hints = result_hint_terms(&records, "What Recall@10 is reported on MS MARCO?", "", 2);
        // r1 and r4 share the metric and the dataset; r3 shares only the dataset; r2 and
        // r5 share nothing.
        assert_eq!(hints, "BM25 MS MARCO Recall@10 SPLADE MS MARCO Recall@10");
        assert_eq!(
            result_hint_terms(&records, "Is the code released?", "", 3),
            ""
        );
        assert_eq!(result_hint_terms(&[], "Recall MS MARCO", "", 3), "");
        assert_eq!(RESULT_HINTS_PER_COLUMN, 3);
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

    #[test]
    fn a_widened_snippet_is_cut_to_the_chunk_that_matched() {
        let before = "The previous chunk ends here.";
        let matched = "Recall@10 of 0.912 on MS MARCO is reported in the matched chunk.";
        let after = "The next chunk begins here.";
        let snippet = format!("{before}\n{matched}\n{after}");
        let start = before.len() + 1;
        let parts = SnippetParts {
            matched: (start, start + matched.len()),
            before: vec![NeighborPart {
                start: 0,
                end: before.len(),
                pages: Some((3, 3)),
            }],
            after: vec![NeighborPart {
                start: start + matched.len() + 1,
                end: snippet.len(),
                pages: Some((5, 5)),
            }],
        };
        let mut metadata = HashMap::new();
        metadata.insert(
            SNIPPET_PARTS_KEY.to_string(),
            serde_json::to_string(&parts).unwrap(),
        );
        metadata.insert("page".to_string(), "4".to_string());

        let text = matched_text(snippet.clone(), &metadata);
        assert_eq!(text, matched);
        // What the neighbours say is not in the passage, so a quote from them cannot be
        // attributed to this chunk, its page and its boxes.
        assert!(!text.contains("previous chunk") && !text.contains("next chunk"));
        let passage = passage_from("hit".into(), text, &metadata);
        assert_eq!((passage.page, passage.text.as_str()), (Some(4), matched));

        // A snippet that was not widened, or whose recorded parts do not describe it, is
        // kept whole.
        assert_eq!(matched_text(snippet.clone(), &HashMap::new()), snippet);
        let mut stale = HashMap::new();
        stale.insert(
            SNIPPET_PARTS_KEY.to_string(),
            serde_json::to_string(&parts).unwrap(),
        );
        assert_eq!(matched_text("short".to_string(), &stale), "short");
    }
}
