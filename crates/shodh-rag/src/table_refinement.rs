//! Background table refinement: PDFs are indexed with the fast heuristic parser first,
//! and those with table-candidate pages are re-parsed with the table model afterwards;
//! when the model structured any table, the file's chunks are replaced.
//!
//! The re-parse ([`refine_file`]) runs without the engine (it is blocking work of about
//! a second per candidate page), and the replacement ([`apply_refinement`]) embeds
//! without the engine lock and only writes when the file is
//! still the one that was re-parsed ([`FileStamp`]) and still indexed, so a refinement
//! never overwrites a newer index of the file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use crate::processing::parser::{DocumentParser, ParsedDocument, MODEL_TABLES_KEY};
use crate::processing::table_model::TableModel;

/// Size and modification time of a file, to tell whether it changed between the
/// re-parse and the replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    pub fn of(path: &Path) -> std::io::Result<FileStamp> {
        let meta = std::fs::metadata(path)?;
        Ok(FileStamp {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// A file re-parsed with the table model.
#[derive(Debug, Clone)]
pub struct RefinedTables {
    pub path: PathBuf,
    pub stamp: FileStamp,
    pub parsed: ParsedDocument,
    /// Tables whose structure came from the model.
    pub model_tables: usize,
    pub parse_ms: u128,
}

/// What applying a refinement did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefineOutcome {
    /// The file's chunks were replaced.
    Replaced { chunks: usize, model_tables: usize },
    /// The file changed since it was re-parsed; its newer index is kept.
    FileChanged,
    /// The file is no longer indexed.
    NotIndexed,
}

/// Re-parses `path` with the table model. `None` when the model structured no table
/// (the fast index already holds everything the file has). Blocking.
pub fn refine_file(path: &Path, model: Arc<TableModel>) -> anyhow::Result<Option<RefinedTables>> {
    let stamp = FileStamp::of(path)?;
    let started = Instant::now();
    let parsed = DocumentParser::with_table_model(model).parse_file(path)?;
    let parse_ms = started.elapsed().as_millis();
    let model_tables = parsed
        .metadata
        .get(MODEL_TABLES_KEY)
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    if model_tables == 0 {
        return Ok(None);
    }
    Ok(Some(RefinedTables {
        path: path.to_path_buf(),
        stamp,
        parsed,
        model_tables,
        parse_ms,
    }))
}

/// Applies `refined` to the shared engine: it is chunked and embedded on a blocking
/// thread without the engine lock, and only stored under the write lock (see
/// [`crate::rag_engine::RAGEngine::commit_refined`]), so searches keep running.
pub async fn apply_refinement(
    rag: &tokio::sync::RwLock<crate::rag_engine::RAGEngine>,
    refined: RefinedTables,
) -> anyhow::Result<RefineOutcome> {
    let (metadata, preparer) = {
        let engine = rag.read().await;
        match engine.refined_metadata(&refined).await? {
            Ok(metadata) => (metadata, engine.file_preparer()?),
            Err(outcome) => return Ok(outcome),
        }
    };
    let RefinedTables {
        path,
        stamp,
        parsed,
        model_tables,
        parse_ms,
    } = refined;
    let file = tokio::task::spawn_blocking(move || {
        preparer.prepare_parsed(&path, parsed, metadata, parse_ms)
    })
    .await??;
    rag.write()
        .await
        .commit_refined(file, stamp, model_tables)
        .await
}

/// Document-level metadata keys the indexer's callers set; they are carried over when
/// a refinement replaces a file's chunks (per-chunk keys such as pages, boxes and
/// headings are recomputed).
const DOCUMENT_KEYS: &[&str] = &[
    "space_id",
    "file_name",
    "filename",
    "file_type",
    "file_extension",
    "doc_type",
    "indexed_at",
    "title",
];

/// The document-level metadata of a stored chunk (see [`DOCUMENT_KEYS`]), with the
/// chunk's space.
pub fn document_metadata(
    stored: &HashMap<String, String>,
    space_id: &str,
) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = DOCUMENT_KEYS
        .iter()
        .filter_map(|k| stored.get(*k).map(|v| (k.to_string(), v.clone())))
        .collect();
    if !space_id.is_empty() {
        out.insert("space_id".to_string(), space_id.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_document_level_metadata_is_carried_over() {
        let stored: HashMap<String, String> = [
            ("space_id", "old"),
            ("file_name", "a.pdf"),
            ("title", "A"),
            ("page_start", "3"),
            ("heading", "Results"),
            ("table_candidate_pages", "3,4"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let kept = document_metadata(&stored, "space-1");
        let mut keys: Vec<&str> = kept.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["file_name", "space_id", "title"]);
        assert_eq!(kept["space_id"], "space-1");
    }

    async fn engine(dir: &Path) -> crate::rag_engine::RAGEngine {
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.join("data");
        config.embedding.model_dir = dir.join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        let mut engine = crate::rag_engine::RAGEngine::new(config).await.unwrap();
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .unwrap();
        engine
    }

    fn table_pdf() -> Vec<u8> {
        use crate::processing::pdf_fixtures::{build_pdf, text};
        build_pdf(
            &[vec![
                text(72.0, 700.0, 10.0, "Results of the two indexes follow."),
                text(72.0, 670.0, 9.0, "Table 1: Recall on SIFT1M."),
                text(72.0, 655.0, 9.0, "Method"),
                text(200.0, 655.0, 9.0, "R@10"),
                text(72.0, 643.0, 9.0, "HNSW"),
                text(200.0, 643.0, 9.0, "95.3"),
            ]],
            None,
        )
    }

    #[tokio::test]
    async fn refinement_replaces_chunks_only_of_the_file_it_reparsed() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = engine(dir.path()).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        engine.set_refinement_queue(tx);
        let path = dir.path().join("paper.pdf");
        std::fs::write(&path, table_pdf()).unwrap();
        let mut metadata = HashMap::new();
        metadata.insert("space_id".to_string(), "space-1".to_string());
        engine
            .add_document_from_file(&path, metadata)
            .await
            .unwrap();
        // The fast index found a table caption: the file is queued for the model.
        assert_eq!(rx.try_recv().ok().as_deref(), Some(path.as_path()));

        // A refinement of the file as it is replaces its chunks, keeping its space.
        let refined = |path: &Path| RefinedTables {
            path: path.to_path_buf(),
            stamp: FileStamp::of(path).unwrap(),
            parsed: DocumentParser::new().parse_file(path).unwrap(),
            model_tables: 1,
            parse_ms: 0,
        };
        // As the worker applies it: embedded without the engine lock.
        let rag = tokio::sync::RwLock::new(engine);
        let outcome = apply_refinement(&rag, refined(&path)).await.unwrap();
        let mut engine = rag.into_inner();
        assert!(matches!(
            outcome,
            RefineOutcome::Replaced {
                model_tables: 1,
                ..
            }
        ));
        let rows = engine.list_documents_raw(None, 100).await.unwrap();
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| r.space_id == "space-1"));
        // Refinement does not queue the file again.
        assert!(rx.try_recv().is_err());

        // The file changed after it was re-parsed: the newer index is kept.
        let stale = refined(&path);
        std::fs::write(&path, [table_pdf(), b"\n%".to_vec()].concat()).unwrap();
        assert_eq!(
            engine.apply_refined_tables(stale).await.unwrap(),
            RefineOutcome::FileChanged
        );

        // A file that is not indexed is left alone.
        let other = dir.path().join("other.pdf");
        std::fs::write(&other, table_pdf()).unwrap();
        assert_eq!(
            engine.apply_refined_tables(refined(&other)).await.unwrap(),
            RefineOutcome::NotIndexed
        );
    }

    #[test]
    fn a_changed_file_has_a_different_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        std::fs::write(&path, b"one").unwrap();
        let before = FileStamp::of(&path).unwrap();
        assert_eq!(FileStamp::of(&path).unwrap(), before);
        std::fs::write(&path, b"longer").unwrap();
        assert_ne!(FileStamp::of(&path).unwrap(), before);
        assert!(FileStamp::of(&dir.path().join("missing.pdf")).is_err());
    }
}
