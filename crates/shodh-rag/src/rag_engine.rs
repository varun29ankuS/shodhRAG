use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use uuid::Uuid;

use crate::config::RAGConfig;
use crate::embeddings::e5::{E5Config, E5Embeddings};
use crate::embeddings::{EmbeddingModel, SearchModelsMissing};
use crate::harness::web::relevance::sigmoid;
use crate::lazy_model::LazyModel;
use crate::processing::chunker::{ContextualChunkResult, DocumentContext, TextChunker};
use crate::processing::ocr::SharedOcrModel;
use crate::processing::parser::{
    DocumentParser, OcrProgress, ParsedDocument, OCR_READER_MISSING_KEY, TABLE_CANDIDATES_KEY,
    TABLE_MODEL_KEY,
};
use crate::processing::structure_chunker::{StructureChunker, STRUCTURE_CHUNKER_VERSION};
use crate::reranking::CrossEncoderReranker;
use crate::search::hybrid::{score_aware_rrf, HybridSource};
use crate::search::TextSearch;
use crate::storage::LanceStore;
use crate::types::{
    ChunkRecord, Citation, ComprehensiveResult, DocumentFormat, MetadataFilter, SimpleSearchResult,
};

/// One document's chunks, embedded and converted to storage records but not
/// yet written. Lets ingestion finish all fallible work before touching the index.
struct PreparedDocument {
    title: String,
    space_id: String,
    records: Vec<ChunkRecord>,
    fts_batch: Vec<(String, String, String, String)>,
    chunk_ids: Vec<Uuid>,
}

/// Parses, chunks and embeds files apart from the engine: copies of its parser,
/// chunkers and embedder (all cheap to clone). Indexing prepares files with it outside
/// the engine lock and takes the lock only to store them ([`RAGEngine::commit_file`]),
/// so searches are not blocked while files are parsed and embedded.
#[derive(Clone)]
pub struct FilePreparer {
    parser: DocumentParser,
    chunker: TextChunker,
    structure_chunker: StructureChunker,
    embeddings: Arc<dyn EmbeddingModel>,
}

/// Directory (under the data directory) of the OCR cache.
pub const OCR_CACHE_DIR: &str = "ocr_cache";

/// A scanned file the indexer read without the scan reader (the PP-OCRv6
/// model, not installed): the system OCR read it, or nothing could. Sent to
/// the queue set with [`RAGEngine::set_scan_reader_queue`] so the file can be
/// read again, with the metadata it was indexed with, once the reader is
/// installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReaderRequest {
    pub path: std::path::PathBuf,
    pub metadata: HashMap<String, String>,
}

/// A file parsed, chunked and embedded, ready for [`RAGEngine::commit_file`].
pub struct PreparedFile {
    path: std::path::PathBuf,
    source: String,
    document: PreparedDocument,
    /// The file has table-candidate pages the table model has not read yet.
    refine: bool,
    /// The file is a scan the system OCR read because the scan reader is not
    /// installed.
    scan_reader_missing: bool,
    /// What the user should know about how the file was read (pages OCR
    /// could not read), when anything.
    notice: Option<String>,
    parse_ms: u128,
    chunk_ms: u128,
    embed_ms: u128,
}

impl PreparedFile {
    /// The file is a scan the system OCR read because the scan reader is not
    /// installed.
    pub fn scan_reader_missing(&self) -> bool {
        self.scan_reader_missing
    }

    /// What the user should know about how the file was read: "pages 4, 9
    /// could not be read" for a scan with pages OCR could not read.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
}

impl FilePreparer {
    /// Parses, chunks and embeds the file at `path` (blocking).
    pub fn prepare(&self, path: &Path, metadata: HashMap<String, String>) -> Result<PreparedFile> {
        self.prepare_with_progress(path, metadata, &|_, _| {})
    }

    /// [`Self::prepare`], reporting OCR's progress through a scanned PDF.
    pub fn prepare_with_progress(
        &self,
        path: &Path,
        metadata: HashMap<String, String>,
        progress: OcrProgress<'_>,
    ) -> Result<PreparedFile> {
        let parse_started = std::time::Instant::now();
        let parsed = self.parser.parse_file_with_progress(path, progress)?;
        let parse_ms = parse_started.elapsed().as_millis();
        self.prepare_parsed(path, parsed, metadata, parse_ms)
    }

    /// Chunks and embeds a parsed file (blocking).
    pub fn prepare_parsed(
        &self,
        path: &Path,
        parsed: ParsedDocument,
        metadata: HashMap<String, String>,
        parse_ms: u128,
    ) -> Result<PreparedFile> {
        let refine = parsed.metadata.contains_key(TABLE_CANDIDATES_KEY)
            && !parsed.metadata.contains_key(TABLE_MODEL_KEY);
        let scan_reader_missing = parsed.metadata.contains_key(OCR_READER_MISSING_KEY);
        let notice = crate::processing::parser::ocr_failed_pages_notice(&parsed.metadata);
        let source = normalize_source_path(path);
        let mut merged_metadata = parsed.metadata;
        for (k, v) in metadata {
            merged_metadata.insert(k, v);
        }
        // Ensure file_path in metadata matches the canonical source used for
        // deletion when the file is stored. This prevents mismatches if the
        // caller passes a differently-formatted path string.
        merged_metadata.insert("file_path".to_string(), source.clone());
        // The identity above is lowercased on Windows; what the user sees
        // keeps the file system's spelling. A spelling the caller kept from
        // an earlier index stands in when the file system gives none.
        let display = on_disk_spelling(path)
            .or_else(|| {
                merged_metadata
                    .get(DISPLAY_PATH_KEY)
                    .filter(|kept| normalize_source_path(Path::new(kept.as_str())) == source)
                    .cloned()
            })
            .unwrap_or_else(|| canonical_path(path).display().to_string());
        merged_metadata.insert(DISPLAY_PATH_KEY.to_string(), display);

        let citation = Citation {
            title: parsed.title.clone(),
            source: source.clone(),
            ..Citation::default()
        };
        // A title the caller gave is kept unless it is only the file's own
        // name: the parser titles a file named by an id by the title
        // printed in it ([`crate::processing::parser::is_identifier_stem`]).
        let stem = path.file_stem().and_then(|s| s.to_str());
        let title = match merged_metadata.get("title") {
            Some(given) if Some(given.as_str()) != stem => given.clone(),
            _ => parsed.title.clone(),
        };

        // Documents parsed into semantic blocks (PDF, LaTeX, Markdown) are
        // chunked by unit: sections, tables with headers, theorems with their
        // proofs, single references. Form fields and relationships extracted
        // from PDFs, and spreadsheet tables, use the section chunker; other
        // formats, and any document those chunkers find no unit in, fall
        // back to sliding windows over its text.
        let chunk_started = std::time::Instant::now();
        let embedder = self.embeddings.as_ref();
        let count = |text: &str| {
            embedder
                .count_tokens(text)
                .unwrap_or_else(|| crate::embeddings::estimate_tokens(text))
        };
        let structured = parsed.document.as_ref().filter(|d| !d.blocks.is_empty());
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        let tabular = crate::processing::tabular::is_spreadsheet_extension(&extension)
            || crate::processing::tabular::is_delimited_extension(&extension);
        let label = source_label(&source);
        let chunk_all = |context: &DocumentContext| {
            let mut chunks = Vec::new();
            if let Some(doc) = structured {
                chunks = self.structure_chunker.chunk(doc, context, &count);
            }
            if !parsed.structured_sections.is_empty() {
                chunks.extend(self.chunker.chunk_structured(
                    &parsed.structured_sections,
                    context,
                    label,
                ));
            }
            if chunks.is_empty() {
                chunks = self
                    .chunker
                    .chunk_with_context(&parsed.content, context, label);
            }
            chunks
        };
        let mut chunks = chunk_in_context(&title, !tabular, &count, &chunk_all);
        if chunks.is_empty() && !parsed.content.trim().is_empty() {
            // Stored as it is, the file would replace its previous chunks
            // with none and count as indexed.
            anyhow::bail!("No chunks were made from the text of {}", path.display());
        }
        // Only structure chunks carry a layout; a document they found no
        // unit in was chunked in windows.
        if chunks.iter().any(|c| c.layout.is_some()) {
            merged_metadata.insert("chunker".to_string(), STRUCTURE_CHUNKER_VERSION.to_string());
        }
        for (index, chunk) in chunks.iter_mut().enumerate() {
            chunk.index = index;
        }
        let chunk_ms = chunk_started.elapsed().as_millis();

        let embed_started = std::time::Instant::now();
        let document = prepare_chunks(
            self.embeddings.as_ref(),
            chunks,
            title,
            source.clone(),
            &merged_metadata,
            &citation,
        )?;
        Ok(PreparedFile {
            path: path.to_path_buf(),
            source,
            document,
            refine,
            scan_reader_missing,
            notice,
            parse_ms,
            chunk_ms,
            embed_ms: embed_started.elapsed().as_millis(),
        })
    }
}

/// The chunks of one document, each with its context ([`DocumentContext`]).
/// `chunk` chunks the document under a given context. The document's
/// opening is taken from the start of its first chunk, as chunked under the
/// title alone; the document is then chunked again under title and opening
/// (the context shrinks what fits of the text). A document of one chunk keeps
/// the title alone: its text holds its opening. `with_opening` is false for
/// spreadsheets and delimited files, whose first words are a header row that
/// each of their chunks repeats already.
fn chunk_in_context(
    title: &str,
    with_opening: bool,
    count_tokens: &dyn Fn(&str) -> usize,
    chunk: &dyn Fn(&DocumentContext) -> Vec<ContextualChunkResult>,
) -> Vec<ContextualChunkResult> {
    let titled = DocumentContext::titled(title);
    let chunks = chunk(&titled);
    if !with_opening {
        return chunks;
    }
    let Some(first) = chunks.first().filter(|_| chunks.len() > 1) else {
        return chunks;
    };
    let context = titled.with_opening(&first.text, count_tokens);
    if context.opening.is_none() {
        return chunks;
    }
    chunk(&context)
}

/// Version of what is indexed for a chunk: its text, the context embedded
/// and keyword-indexed with it, and the metadata search reads. Stored with
/// every chunk (`index_format`); chunks without it are format 1. Raise it
/// whenever any of these change, so that what was indexed before is found
/// and indexed again instead of being ranked against chunks made the new way.
///
/// 2: chunks carry the document's opening in their context, and the context
/// is stored (`context`) for the reranker and for rebuilding the keyword index.
/// 3: contexts name a file by its name, never its folders ([`source_label`]),
/// and carry only the names in the document's opening, on every chunk.
///
/// A change to the chunk text itself (what is read from a file and how it
/// is cut) raises [`CHUNKER_VERSION`] instead: a chunk of an older format is
/// brought up to date from its stored text where it can be
/// ([`RAGEngine::upgrade_in_place`]), which a change of the text cannot be.
pub const INDEX_FORMAT: u32 = 3;

/// Chunk metadata key: the [`INDEX_FORMAT`] a chunk was indexed with.
pub const INDEX_FORMAT_KEY: &str = "index_format";

/// Version of how a file's text is read and cut into chunks (the parsers,
/// [`TextChunker`] and [`StructureChunker`]). Stored with every chunk
/// (`chunker_version`); chunks without it are version 1. Raise it whenever
/// the chunk text made from the same file changes: files whose chunks are of
/// an older version are read and chunked again in full
/// ([`RAGEngine::rechunk_sources`]), never upgraded in place from their old
/// text.
///
/// 1: the chunkers before the parsing fixes (labelled `structure-v1`).
/// 2: chunking always makes progress and never drops short text, CJK or
/// one-word units; a page too short for a chunk joins its neighbour; no
/// redundant tail chunks; HTML is read on byte offsets with comments, CDATA
/// and C1 references handled; `.htm` files are indexed.
pub const CHUNKER_VERSION: u32 = 2;

/// Chunk metadata key: the [`CHUNKER_VERSION`] a chunk was made with, or,
/// for a chunk kept on purpose ([`CHUNKS_KEPT_FROM_KEY`]), the version it was
/// last looked at under.
pub const CHUNKER_VERSION_KEY: &str = "chunker_version";

/// Chunk metadata key: on the chunks of a document kept from an older
/// chunker on purpose ([`RAGEngine::keep_chunks_of_older_chunker`]), the
/// [`CHUNKER_VERSION`] their text was made with. They are not re-read until
/// their file changes or the version is raised again.
pub const CHUNKS_KEPT_FROM_KEY: &str = "chunks_kept_from";

/// The file, in the data directory, that lists the replacements of files a
/// commit began and has not finished ([`RAGEngine::commit_file`]).
pub const REPLACED_CHUNKS_FILE: &str = "replaced_chunks.json";

/// The file, in the data directory, that lists the files removed from the index
/// whose data the [`LibraryRemovalObserver`] has not confirmed forgetting
/// ([`RAGEngine::settle_forgotten`]).
pub const PENDING_FORGETS_FILE: &str = "pending_forgets.json";

/// Something that keeps data about indexed files and must lose it when a file leaves
/// the library (the evidence tables: their rows, cells and cached quotes). Every
/// removal from the index reports the files that are gone through
/// [`RAGEngine::set_removal_observer`], so no removal can skip it.
pub trait LibraryRemovalObserver: Send + Sync {
    /// Forgets everything kept about `sources`, given as the index spells them
    /// ([`normalize_source_path`]). Must be safe to repeat.
    fn forget_sources(&self, sources: &[String]) -> Result<()>;

    /// Forgets everything kept about every file (the index was reset). Must be safe
    /// to repeat.
    fn forget_all(&self) -> Result<()>;
}

/// The entry of [`PENDING_FORGETS_FILE`] that stands for "every file": a reset whose
/// files could not be listed first. `<` and `>` are not in a Windows file name.
const FORGET_ALL: &str = "<all>";

/// Whether `source` is a file path. A `scheme://` source (a calendar item, a note) is
/// not a paper and is never a row of an evidence table.
fn is_file_source(source: &str) -> bool {
    !source.contains("://")
}

/// The file sources among `sources`, each once.
fn file_sources(sources: impl IntoIterator<Item = String>) -> Vec<String> {
    sources
        .into_iter()
        .filter(|s| is_file_source(s) && s != FORGET_ALL)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The removals written down and not confirmed. A record that does not parse is not
/// dropped: it is moved to `pending_forgets.corrupt-<unix seconds>.json` and logged, so
/// nothing is overwritten and the entries can be read by hand. When it cannot be moved
/// the read fails, and nothing is written over it.
fn load_pending_forgets(path: &Path) -> Result<std::collections::BTreeSet<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(e) => return Err(e.into()),
    };
    match serde_json::from_str(&text) {
        Ok(pending) => Ok(pending),
        Err(parse) => {
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let aside = path.with_file_name(format!("pending_forgets.corrupt-{seconds}.json"));
            std::fs::rename(path, &aside).with_context(|| {
                format!("the pending removals do not parse ({parse}) and could not be moved aside")
            })?;
            tracing::error!(
                error = %parse,
                moved_to = %aside.display(),
                "the pending removals did not parse; kept aside, and the record starts empty"
            );
            Ok(Default::default())
        }
    }
}

/// Writes `sources` as the removals not confirmed (none: the record goes), through a
/// temporary file renamed over the previous record.
fn write_pending_forgets(path: &Path, sources: &std::collections::BTreeSet<String>) -> Result<()> {
    if sources.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut out = std::fs::File::create(&tmp)?;
        out.write_all(&serde_json::to_vec(sources)?)?;
        // On disk before the rename, so a power loss can't leave an empty record.
        out.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// A replacement of a file's chunks in progress: written down before the new
/// chunks are stored, dropped once the old ones are gone
/// ([`RAGEngine::settle_replaced`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Replacement {
    /// The file (its index identity).
    source: String,
    /// The ids of the chunks it had before.
    previous_ids: Vec<String>,
}

/// Chunk metadata key: the context the chunk was embedded and keyword-indexed
/// with (see [`ContextualChunkResult::context`]).
pub const CHUNK_CONTEXT_KEY: &str = "context";

/// The context the cross-encoder reads a result's passage with: the context
/// the chunk was embedded with. A chunk indexed before the context was
/// stored ([`INDEX_FORMAT`] 1) is read with its document's title and section,
/// as it was embedded then, until it is indexed again.
fn rerank_context(result: &ComprehensiveResult) -> String {
    if let Some(context) = result.metadata.get(CHUNK_CONTEXT_KEY) {
        return context.clone();
    }
    let mut context = format!("Document: \"{}\". ", result.citation.title.trim());
    if let Some(heading) = result
        .metadata
        .get("heading")
        .map(|h| h.trim())
        .filter(|h| !h.is_empty())
    {
        context.push_str("Section: ");
        context.push_str(heading);
        context.push_str(". ");
    }
    context
}

/// How a chunk's context names the file `source`: its file name. The folders
/// a file sits in are the user's, not the document's: in what is embedded,
/// keyword-indexed and reranked they would be words of every chunk of every
/// file there. A `scheme://` source (a calendar item) is kept as it is.
fn source_label(source: &str) -> &str {
    if source.contains("://") {
        return source;
    }
    source.rsplit(['/', '\\']).next().unwrap_or(source)
}

/// A LanceDB filter for the chunks of `source`.
fn source_predicate(source: &str) -> String {
    format!("source = '{}'", source.replace('\'', "''"))
}

/// A LanceDB filter for the chunks that name no OCR engine. serde_json writes
/// keys without spaces and escapes a quote inside a value, so only the key
/// itself matches.
fn no_ocr_engine_recorded() -> String {
    format!(
        "metadata_json NOT LIKE '%\"{}\":\"%'",
        crate::processing::parser::OCR_ENGINE_KEY
    )
}
/// A LanceDB filter for the chunks not indexed with [`INDEX_FORMAT`]. The
/// metadata is serialized by serde_json (no spaces), so the current format
/// appears verbatim as `"index_format":"<n>"`; inside a metadata value its
/// quotes would be escaped, so a value can never match.
fn current_format_excluded() -> String {
    format!("metadata_json NOT LIKE '%\"{INDEX_FORMAT_KEY}\":\"{INDEX_FORMAT}\"%'")
}

/// A LanceDB filter for the chunks not made with [`CHUNKER_VERSION`], matched
/// as [`current_format_excluded`] matches the format.
fn current_chunker_excluded() -> String {
    format!("metadata_json NOT LIKE '%\"{CHUNKER_VERSION_KEY}\":\"{CHUNKER_VERSION}\"%'")
}

/// Embed chunks and build the storage records for one document without
/// touching the stores. Fails (with nothing written) if embedding fails.
fn prepare_chunks(
    embedder: &dyn EmbeddingModel,
    chunks: Vec<ContextualChunkResult>,
    title: String,
    source: String,
    metadata: &HashMap<String, String>,
    citation: &Citation,
) -> Result<PreparedDocument> {
    let space_id = metadata.get("space_id").cloned().unwrap_or_default();
    if chunks.is_empty() {
        return Ok(PreparedDocument {
            title,
            space_id,
            records: Vec::new(),
            fts_batch: Vec::new(),
            chunk_ids: Vec::new(),
        });
    }

    // Embed the contextualized text (with document context prefix) for better
    // vector representation
    let contextualized: Vec<String> = chunks.iter().map(|c| c.contextualized_text()).collect();
    let chunk_texts: Vec<&str> = contextualized.iter().map(String::as_str).collect();
    let embeddings = embedder.embed_documents(&chunk_texts)?;

    let doc_id = Uuid::new_v4();
    let metadata_json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string());
    let now = chrono::Utc::now().timestamp();

    let mut records = Vec::with_capacity(chunks.len());
    let mut fts_batch = Vec::with_capacity(chunks.len());
    let mut chunk_ids = Vec::with_capacity(chunks.len());

    for (i, ((chunk, embedding), contextualized)) in chunks
        .iter()
        .zip(embeddings)
        .zip(contextualized)
        .enumerate()
    {
        let chunk_id = chunk.id;
        chunk_ids.push(chunk_id);

        let mut per_chunk_meta = metadata.clone();
        per_chunk_meta.insert(INDEX_FORMAT_KEY.to_string(), INDEX_FORMAT.to_string());
        per_chunk_meta.insert(CHUNKER_VERSION_KEY.to_string(), CHUNKER_VERSION.to_string());
        per_chunk_meta.insert(CHUNK_CONTEXT_KEY.to_string(), chunk.context.clone());
        if let Some(heading) = &chunk.heading {
            per_chunk_meta.insert("chunk_type".to_string(), heading.clone());
            per_chunk_meta.insert("heading".to_string(), heading.clone());
        }
        match &chunk.layout {
            Some(layout) => insert_layout_metadata(&mut per_chunk_meta, layout),
            None => insert_page_metadata(&mut per_chunk_meta, chunk.page, chunk.page_end),
        }
        // Extract structured fields (emails, phones, etc.) at ingest time
        for (k, v) in extract_structured_fields(&chunk.text) {
            per_chunk_meta.insert(k, v);
        }
        let per_chunk_meta_json =
            serde_json::to_string(&per_chunk_meta).unwrap_or_else(|_| metadata_json.clone());

        // Citation is stored per chunk so its page survives into search results.
        let chunk_citation = Citation {
            page_numbers: page_numbers_from_metadata(&per_chunk_meta),
            ..citation.clone()
        };
        let citation_json =
            serde_json::to_string(&chunk_citation).unwrap_or_else(|_| "{}".to_string());

        // Store the original text (without context prefix) for display
        records.push(ChunkRecord {
            id: chunk_id.to_string(),
            doc_id: doc_id.to_string(),
            chunk_index: i as u32,
            text: chunk.text.clone(),
            title: title.clone(),
            source: source.clone(),
            heading: chunk.heading.clone().unwrap_or_default(),
            vector: embedding,
            space_id: space_id.clone(),
            metadata_json: per_chunk_meta_json,
            citation_json,
            created_at: now,
        });

        // Index contextualized text in FTS for richer BM25 matching
        fts_batch.push((
            chunk_id.to_string(),
            contextualized,
            title.clone(),
            source.clone(),
        ));
    }

    Ok(PreparedDocument {
        title,
        space_id,
        records,
        fts_batch,
        chunk_ids,
    })
}

/// One native spelling of a path, applied where paths enter the indexer
/// (folder walks, single files, uploads): the platform separator throughout
/// (a folder typed as `C:/Papers` and walked to `C:/Papers\a.pdf` becomes
/// `C:\Papers\a.pdf`), no verbatim `\\?\` prefix, no `.` components,
/// `..` resolved lexically, no trailing separator. Case is preserved; the
/// file system is not consulted.
pub fn canonical_path(path: &Path) -> std::path::PathBuf {
    let raw = path.to_string_lossy();
    let mut text = raw.to_string();
    for (verbatim, replacement) in [
        ("\\\\?\\UNC\\", "\\\\"),
        ("//?/UNC/", "//"),
        ("\\\\?\\", ""),
        ("//?/", ""),
    ] {
        if let Some(rest) = text.strip_prefix(verbatim) {
            text = format!("{replacement}{rest}");
            break;
        }
    }
    if cfg!(windows) {
        text = text.replace('/', "\\");
    }
    let mut out = std::path::PathBuf::new();
    for component in Path::new(&text).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let ends_in_name = matches!(
                    out.components().next_back(),
                    Some(std::path::Component::Normal(_))
                );
                if ends_in_name {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The identity of an indexed file: its [`canonical_path`] with forward
/// slashes, lowercased on Windows (whose file systems are case-insensitive).
/// Stored as each chunk's `source`, so `delete_by_source` and re-indexing
/// match however the path was spelled. Rows written by older versions under
/// another spelling are removed on re-index (see `legacy_source_spellings`).
pub fn normalize_source_path(path: &Path) -> String {
    let s = canonical_path(path)
        .display()
        .to_string()
        .replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// Chunk metadata key: the indexed file's path as the file system spells it
/// (see [`on_disk_spelling`]), for display. The chunk's `source` and
/// `file_path` stay the identity ([`normalize_source_path`]).
pub const DISPLAY_PATH_KEY: &str = "display_path";

/// The names of a document's file a chunk's metadata keeps
/// ([`RAGEngine::document_names`]); blank values read as absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct DocumentNames {
    /// [`DISPLAY_PATH_KEY`]: the path as the file system spelled it at indexing.
    #[serde(default, deserialize_with = "non_blank")]
    pub display_path: Option<String>,
    /// The file name in its original case.
    #[serde(default, deserialize_with = "non_blank")]
    pub filename: Option<String>,
}

fn non_blank<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    let value: Option<String> = serde::Deserialize::deserialize(deserializer)?;
    Ok(value.filter(|v| !v.trim().is_empty()))
}

/// The path of the file at `path` as the file system spells it, in its
/// [`canonical_path`] form: on Windows the case each folder and the file
/// have on disk, however `path` was cased; elsewhere `path` itself, whose
/// case is already the file's. `None` on Windows when the file cannot be
/// resolved (gone, unreadable), or when resolving it reaches the file under
/// another identity (a short `8.3` name expanded, a mapped drive or a
/// junction followed): only a spelling that differs from `path` in case is
/// returned, so it never names the file differently from the index.
pub fn on_disk_spelling(path: &Path) -> Option<String> {
    if !cfg!(windows) {
        return Some(canonical_path(path).display().to_string());
    }
    let resolved = canonical_path(&std::fs::canonicalize(path).ok()?);
    (normalize_source_path(&resolved) == normalize_source_path(path))
        .then(|| resolved.display().to_string())
}

// Compiled-once regex patterns for structured field extraction at ingest time.
// Extracting emails, phones, PAN, GSTIN, amounts, and dates from chunk text
// at zero query-time cost — no LLM needed for field-level extraction.
static RE_EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}").unwrap());
static RE_PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\+91[\s\-]?)?(?:\d[\s\-]?){10}").unwrap());
static RE_PAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Z]{5}\d{4}[A-Z]\b").unwrap());
static RE_GSTIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{2}[A-Z]{5}\d{4}[A-Z]\d[Z][A-Z0-9]\b").unwrap());
static RE_AMOUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:Rs\.?|INR|₹)\s*[\d,]+(?:\.\d{1,2})?").unwrap());
static RE_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{1,2}[/\-\.]\d{1,2}[/\-\.]\d{2,4}\b").unwrap());
static RE_INVOICE_NO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:invoice|inv|bill)[\s.#:_\-]*(?:no\.?|number)?[\s.#:_\-]*([A-Z0-9/\-]+)")
        .unwrap()
});

/// Extract structured fields from chunk text using regex.
/// Returns key-value pairs to merge into chunk metadata.
/// This runs once at ingest time — zero cost at query time.
/// Record a chunk's source pages in its metadata. Paged chunks get `page`,
/// `page_start` and `page_end`: equal for a chunk of one page, a range for a
/// chunk whose text runs onto a later page (a short page merged with its
/// neighbour, `page_end`); the range keys let consumers treat merged/expanded
/// results uniformly. Unpaged chunks get no page keys.
fn insert_page_metadata(
    meta: &mut HashMap<String, String>,
    page: Option<usize>,
    page_end: Option<usize>,
) {
    if let Some(page) = page {
        let start = page.to_string();
        let end = page_end
            .filter(|&end| end > page)
            .unwrap_or(page)
            .to_string();
        meta.insert("page".to_string(), start.clone());
        meta.insert("page_start".to_string(), start);
        meta.insert("page_end".to_string(), end);
    }
}

/// Record a structured chunk's provenance: page range, bounding boxes (JSON
/// `[{"page":3,"x0":..,"y0":..,"x1":..,"y1":..}]`, PDF points, bottom-left
/// origin), section path (`" > "`-joined), block kinds and unit kind.
fn insert_layout_metadata(
    meta: &mut HashMap<String, String>,
    layout: &crate::processing::structure_chunker::ChunkLayout,
) {
    if let Some(start) = layout.page_start {
        let end = layout.page_end.unwrap_or(start);
        meta.insert("page".to_string(), start.to_string());
        meta.insert("page_start".to_string(), start.to_string());
        meta.insert("page_end".to_string(), end.to_string());
    }
    if !layout.regions.is_empty() {
        let regions: Vec<_> = layout
            .regions
            .iter()
            .map(|r| crate::processing::structure_chunker::ChunkRegion {
                page: r.page,
                bbox: r.bbox.rounded(),
            })
            .collect();
        if let Ok(json) = serde_json::to_string(&regions) {
            meta.insert("bboxes".to_string(), json);
        }
    }
    if !layout.section_path.is_empty() {
        meta.insert("section_path".to_string(), layout.section_path.join(" > "));
    }
    meta.insert("block_kinds".to_string(), layout.block_kinds.join(","));
    meta.insert("unit_kind".to_string(), layout.unit.to_string());
    if layout.incomplete {
        meta.insert(TABLE_INCOMPLETE_KEY.to_string(), "true".to_string());
    }
}

/// Chunk metadata flag: the chunk holds a table whose cells missed part of its
/// region's text, or that region's raw text.
pub const TABLE_INCOMPLETE_KEY: &str = "table_incomplete";

/// Whether a search result should be widened with its neighbouring chunks.
/// Window chunks cut text mid-thought, so their neighbours restore context.
/// Structure chunks (`unit_kind` set) are complete units — a section's
/// paragraphs, a table, a theorem, one bibliography entry — and their page
/// and section labels describe exactly their own text; widening them would
/// cross headings and pages and merge adjacent references.
fn wants_neighbor_expansion(meta: &HashMap<String, String>) -> bool {
    !meta.contains_key("unit_kind")
}

/// Metadata key of a result widened with its neighbouring chunks: where the
/// chunk that matched and each neighbour lie in its snippet ([`SnippetParts`],
/// as JSON). Readers that cut the snippet use it to keep the matched chunk.
pub const SNIPPET_PARTS_KEY: &str = "snippet_parts";

/// A neighbouring chunk joined to a result's snippet: its byte range in the
/// snippet and the pages it covers (`None` for an unpaged chunk).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NeighborPart {
    pub start: usize,
    pub end: usize,
    #[serde(default)]
    pub pages: Option<(u32, u32)>,
}

/// The parts of a snippet widened with neighbours, as byte ranges in it: the
/// chunk that matched, and the neighbours before and after it in reading order.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SnippetParts {
    pub matched: (usize, usize),
    pub before: Vec<NeighborPart>,
    pub after: Vec<NeighborPart>,
}

impl SnippetParts {
    /// The parts `meta` records for `snippet`, when they describe it: every
    /// range on char boundaries inside it, in reading order, none overlapping.
    /// `None` for a snippet that was not widened.
    pub fn of(snippet: &str, meta: &HashMap<String, String>) -> Option<Self> {
        let parts: Self = serde_json::from_str(meta.get(SNIPPET_PARTS_KEY)?).ok()?;
        let ranges = parts
            .before
            .iter()
            .map(|p| (p.start, p.end))
            .chain(std::iter::once(parts.matched))
            .chain(parts.after.iter().map(|p| (p.start, p.end)));
        let mut reached = 0;
        for (start, end) in ranges {
            if start < reached
                || end < start
                || end > snippet.len()
                || !snippet.is_char_boundary(start)
                || !snippet.is_char_boundary(end)
            {
                return None;
            }
            reached = end;
        }
        Some(parts)
    }
}

/// The pages a chunk covers, from the metadata written at indexing time
/// (`page_start`/`page_end`, else `page`), lowest first. `None` for an
/// unpaged chunk.
pub fn page_range_from_metadata(meta: &HashMap<String, String>) -> Option<(u32, u32)> {
    let read = |key: &str| meta.get(key).and_then(|v| v.trim().parse::<u32>().ok());
    match (read("page_start"), read("page_end")) {
        (Some(start), Some(end)) => Some((start.min(end), start.max(end))),
        (Some(one), None) | (None, Some(one)) => Some((one, one)),
        (None, None) => read("page").map(|p| (p, p)),
    }
}

/// Appends `piece` to `text`, a line break after what is there, and returns
/// the byte range it took.
fn push_part(text: &mut String, piece: &str) -> (usize, usize) {
    if !text.is_empty() {
        text.push('\n');
    }
    let start = text.len();
    text.push_str(piece);
    (start, text.len())
}

/// Appends `piece`, the text of neighbour `hit` or part of it, to `text`
/// ([`push_part`]) and returns its part.
fn push_neighbor(text: &mut String, piece: &str, hit: &crate::storage::SearchHit) -> NeighborPart {
    let (start, end) = push_part(text, piece);
    let pages = serde_json::from_str::<HashMap<String, String>>(&hit.metadata_json)
        .ok()
        .and_then(|meta| page_range_from_metadata(&meta));
    NeighborPart { start, end, pages }
}

/// Shortest repeat of one chunk's end at the next one's start that is taken
/// for the window overlap, in characters: a shorter one ("the", a number) is
/// as likely a coincidence of two different sentences.
const MIN_OVERLAP_CHARS: usize = 12;

/// Length in bytes of the longest end of `first` that `second` starts with
/// (sliding windows repeat up to `chunk_overlap` bytes of a chunk at the start
/// of the next), when it is at least [`MIN_OVERLAP_CHARS`] long; else 0. On a
/// char boundary of both.
fn overlap_len(first: &str, second: &str) -> usize {
    let longest = first.len().min(second.len());
    (1..=longest)
        .rev()
        .filter(|&len| second.is_char_boundary(len))
        .find(|&len| first.ends_with(&second[..len]))
        .filter(|&len| second[..len].chars().count() >= MIN_OVERLAP_CHARS)
        .unwrap_or(0)
}

/// `core`, the text of chunk `chunk_index`, with the `neighbors` of the same
/// document joined around it in reading order, one line break apart, and
/// where each part lies in the joined text. Text a neighbour repeats of the
/// piece next to it (the windows' overlap) is left out of the neighbour.
fn join_neighbors(
    core: &str,
    chunk_index: u32,
    neighbors: &[crate::storage::SearchHit],
) -> (String, SnippetParts) {
    let mut ordered: Vec<&crate::storage::SearchHit> = neighbors
        .iter()
        .filter(|hit| hit.chunk_index != chunk_index)
        .collect();
    ordered.sort_by_key(|hit| hit.chunk_index);
    let before: Vec<&crate::storage::SearchHit> = ordered
        .iter()
        .copied()
        .filter(|hit| hit.chunk_index < chunk_index)
        .collect();
    let after: Vec<&crate::storage::SearchHit> = ordered
        .iter()
        .copied()
        .filter(|hit| hit.chunk_index > chunk_index)
        .collect();
    let mut text = String::new();
    let mut parts = SnippetParts::default();
    for (i, hit) in before.iter().enumerate() {
        let next = before.get(i + 1).map_or(core, |h| h.text.as_str());
        let piece = &hit.text[..hit.text.len() - overlap_len(&hit.text, next)];
        parts.before.push(push_neighbor(&mut text, piece, hit));
    }
    parts.matched = push_part(&mut text, core);
    for (i, hit) in after.iter().enumerate() {
        let previous = match i {
            0 => core,
            _ => after[i - 1].text.as_str(),
        };
        let piece = &hit.text[overlap_len(previous, &hit.text)..];
        parts.after.push(push_neighbor(&mut text, piece, hit));
    }
    (text, parts)
}

/// Whether a chunk is one bibliography entry.
fn is_reference_entry(meta: &HashMap<String, String>) -> bool {
    meta.get("unit_kind").map(String::as_str) == Some("reference_entry")
}

/// Whether `query` asks about citations or the works behind an idea ("which paper
/// proposed ...", "what does X cite"). Only such queries are answered from
/// bibliography entries; any other query gets content chunks only.
pub fn seeks_references(query: &str) -> bool {
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let citation_word = words.iter().any(|w| {
        w.starts_with("cite")
            || w.starts_with("citing")
            || w.starts_with("citation")
            || w.starts_with("referenc")
            || w.starts_with("bibliograph")
    });
    let asks_for_origin = words.windows(2).any(|pair| {
        matches!(
            (pair[0], pair[1]),
            (
                "who",
                "proposed" | "introduced" | "invented" | "first" | "wrote" | "authored"
            ) | (
                "which" | "what",
                "paper" | "papers" | "work" | "works" | "article" | "articles"
            ) | ("original", "paper" | "work")
        )
    });
    citation_word || asks_for_origin
}

/// Other spellings under which older versions of the indexer may have stored
/// `path`: the verbatim string, its forward-slash form, and the previous
/// normalization (forward slashes, lowercased on Windows, `\\?\` prefixes and
/// `.` components left in place). Excludes `canonical` itself.
fn legacy_source_spellings(path: &Path, canonical: &str) -> Vec<String> {
    let verbatim = path.display().to_string();
    let slashed = verbatim.replace('\\', "/");
    let previous = if cfg!(windows) {
        slashed.to_lowercase()
    } else {
        slashed.clone()
    };
    let mut out = vec![verbatim, slashed, previous];
    out.retain(|s| s != canonical);
    out.sort();
    out.dedup();
    out
}

/// The owners (`space_id`) of files added in chats start with this: such
/// files belong to no Library folder, so removing a folder never removes
/// them, and to where they were added ([`chat_uploads_source`]), so a
/// workspace's chats see only their own.
pub const CHAT_UPLOADS_PREFIX: &str = "chat-uploads:";

/// The owner of files added in a chat outside every workspace (the main Ask).
pub const CHAT_UPLOADS_MAIN: &str = "chat-uploads:main";

/// The owner an earlier build of this change gave every chat upload; such
/// files are the main Ask's ([`RAGEngine::hand_over_chat_uploads`]).
#[cfg(test)]
const LEGACY_CHAT_UPLOADS: &str = "chat-uploads";

/// The owner of a file added in a chat of `workspace` (`None`: the main
/// Ask): `chat-uploads:<workspace id>`, or [`CHAT_UPLOADS_MAIN`]. `None` for
/// a workspace id that cannot be one (empty, too long, a character other
/// than letters, digits, `-` and `_`, or `main`, which names the main Ask).
pub fn chat_uploads_source(workspace: Option<&str>) -> Option<String> {
    let Some(id) = workspace else {
        return Some(CHAT_UPLOADS_MAIN.to_string());
    };
    let plain = !id.is_empty()
        && id.len() <= 200
        && id != "main"
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| format!("{CHAT_UPLOADS_PREFIX}{id}"))
}

/// Whether `space_id` owns files added in chats rather than being a source
/// of the Library: such an owner is never removed or indexed as a folder.
pub fn is_chat_uploads(space_id: &str) -> bool {
    space_id == "chat-uploads" || space_id.starts_with(CHAT_UPLOADS_PREFIX)
}

/// Whether chunk metadata (`metadata_json`) is that of a file added in a
/// chat: only the chat upload records `uploaded_at`.
fn added_in_chat(metadata_json: &str) -> bool {
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(metadata_json)
        .is_ok_and(|metadata| metadata.contains_key("uploaded_at"))
}

/// The spellings under which an indexer may have stored the file or folder
/// `path` ([`normalize_source_path`] and [`legacy_source_spellings`]),
/// without a trailing separator.
fn source_spellings(path: &Path) -> Vec<String> {
    let canonical = normalize_source_path(path);
    let mut spellings = vec![canonical.clone()];
    spellings.extend(legacy_source_spellings(path, &canonical));
    spellings
        .into_iter()
        .map(|s| s.trim_end_matches(['/', '\\']).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether the stored `source` is one of `spellings` (a file) or lies under
/// one of them (a folder), across a separator: `c:/research/papers` holds
/// `c:/research/papers/a.pdf`, never `c:/research/papers 2024/a.pdf`. Plain
/// string comparison, so no character in a path acts as a wildcard.
fn lies_under(source: &str, spellings: &[String]) -> bool {
    spellings
        .iter()
        .any(|folder| match source.strip_prefix(folder.as_str()) {
            Some(rest) => rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\'),
            None => false,
        })
}

/// Human-readable page label (`"3"` or `"3-5"`) from chunk metadata written by
/// [`insert_page_metadata`]. Returns `None` for unpaged chunks.
pub fn page_numbers_from_metadata(meta: &HashMap<String, String>) -> Option<String> {
    let non_empty = |key: &str| meta.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
    match (non_empty("page_start"), non_empty("page_end")) {
        (Some(start), Some(end)) if start != end => Some(format!("{}-{}", start, end)),
        (Some(start), _) => Some(start.to_string()),
        (None, Some(end)) => Some(end.to_string()),
        (None, None) => non_empty("page").map(str::to_string),
    }
}

fn extract_structured_fields(text: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();

    // Emails
    let emails: Vec<String> = RE_EMAIL
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !emails.is_empty() {
        fields.insert("extracted_emails".to_string(), emails.join(", "));
    }

    // Phone numbers (basic cleanup: strip non-digit noise)
    let phones: Vec<String> = RE_PHONE
        .find_iter(text)
        .map(|m| {
            m.as_str()
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '+')
                .collect::<String>()
        })
        .filter(|p| p.len() >= 10)
        .collect();
    if !phones.is_empty() {
        fields.insert("extracted_phones".to_string(), phones.join(", "));
    }

    // PAN numbers
    let pans: Vec<String> = RE_PAN
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !pans.is_empty() {
        fields.insert("extracted_pan".to_string(), pans.join(", "));
    }

    // GSTIN
    let gstins: Vec<String> = RE_GSTIN
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !gstins.is_empty() {
        fields.insert("extracted_gstin".to_string(), gstins.join(", "));
    }

    // Amounts
    let amounts: Vec<String> = RE_AMOUNT
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !amounts.is_empty() {
        fields.insert("extracted_amounts".to_string(), amounts.join("; "));
    }

    // Dates
    let dates: Vec<String> = RE_DATE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !dates.is_empty() {
        fields.insert("extracted_dates".to_string(), dates.join(", "));
    }

    // Invoice numbers
    if let Some(cap) = RE_INVOICE_NO.captures(text) {
        if let Some(inv) = cap.get(1) {
            fields.insert("extracted_invoice_no".to_string(), inv.as_str().to_string());
        }
    }

    fields
}

/// The ONNX models the engine searches with: the E5 embedder (required) and
/// the cross-encoder reranker (optional). Both are only located here (cheap);
/// they load on first use and are dropped again when idle (see
/// [`SharedEmbedder`], [`SharedReranker`]). The E5 model reads ~600 MB, so an
/// app that is only opened, or only browses its library, never holds it.
pub struct SearchModels {
    embeddings: EmbedderSource,
    reranker_dir: Option<std::path::PathBuf>,
    /// The PP-OCRv6 model directory, when the model is installed.
    ocr_dir: Option<std::path::PathBuf>,
}

enum EmbedderSource {
    /// The installed E5 model, loaded on first use.
    E5(E5Config),
    /// A model that is already loaded (tests).
    Loaded(Arc<dyn EmbeddingModel>),
}

impl SearchModels {
    /// Models over an embedder that is already loaded (the app's tests use
    /// a deterministic one), without a reranker or OCR model.
    pub fn from_embedder(embeddings: Arc<dyn EmbeddingModel>) -> Self {
        Self {
            embeddings: EmbedderSource::Loaded(embeddings),
            reranker_dir: None,
            ocr_dir: None,
        }
    }

    /// Whether the E5 model files exist under `config.embedding.model_dir`.
    pub fn available(config: &RAGConfig) -> bool {
        E5Config::auto_detect(&config.embedding.model_dir).is_some()
    }

    /// Locate the models under `config.embedding.model_dir` (nothing is
    /// loaded). Fails with [`SearchModelsMissing`] when the E5 files are absent.
    pub fn load(config: &RAGConfig) -> Result<Self> {
        let e5_config =
            E5Config::auto_detect(&config.embedding.model_dir).ok_or(SearchModelsMissing)?;

        let reranker_dir = if config.features.enable_reranking
            || config.features.enable_cross_encoder
        {
            let dir = config.embedding.model_dir.join("ms-marco-MiniLM-L6-v2");
            match CrossEncoderReranker::check_files(&dir) {
                Ok(()) => Some(dir),
                Err(e) => {
                    tracing::warn!("Reranker not available ({e}), continuing without reranking");
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            embeddings: EmbedderSource::E5(e5_config),
            reranker_dir,
            ocr_dir: Self::locate_ocr(config),
        })
    }

    /// The installed OCR model's directory under `config.embedding.model_dir`
    /// (optional: without it scans go to the system OCR).
    fn locate_ocr(config: &RAGConfig) -> Option<std::path::PathBuf> {
        let dir = crate::processing::ocr::model_dir(&config.embedding.model_dir);
        crate::processing::ocr::is_installed(&dir).then_some(dir)
    }

    pub fn dimension(&self) -> usize {
        match &self.embeddings {
            EmbedderSource::E5(config) => config.dimension,
            EmbedderSource::Loaded(model) => model.dimension(),
        }
    }

    pub fn has_reranker(&self) -> bool {
        self.reranker_dir.is_some()
    }
}

/// The engine's E5 embedder, loaded on first use and dropped when idle;
/// shared so the idle unloader and the install check reach it without the
/// engine lock.
pub type SharedEmbedder = Arc<LazyModel<E5Embeddings>>;

/// [`EmbeddingModel`] over the lazily loaded E5 model: the first call loads
/// it (seconds, on the calling thread), later calls reuse it until it has
/// been idle long enough to be dropped.
struct LazyEmbeddings {
    model: SharedEmbedder,
    dimension: usize,
}

impl LazyEmbeddings {
    fn loaded(&self) -> Result<Arc<E5Embeddings>> {
        self.model.get().ok_or_else(|| {
            anyhow::anyhow!(
                "The search model could not be loaded (see the log); reinstall it in Settings → Search"
            )
        })
    }
}

impl EmbeddingModel for LazyEmbeddings {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.loaded()?.embed_query(text)
    }

    fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        self.loaded()?.embed_document(text)
    }

    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // Text too short to chunk (a one-line calendar item) reaches here
        // with no passages: never load the model for nothing.
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.loaded()?.embed_documents(texts)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn count_tokens(&self, text: &str) -> Option<usize> {
        self.model.get()?.count_tokens(text)
    }
}

/// The engine's cross-encoder, shared so other rankers use the model without
/// taking the engine lock. Loaded on first use and dropped when idle; not
/// available until the search models are attached, or when reranking is off.
pub type SharedReranker = Arc<LazyModel<CrossEncoderReranker>>;

pub struct RAGEngine {
    store: LanceStore,
    text_search: TextSearch,
    /// `None` until the search models are installed and attached; search and
    /// indexing then fail with [`SearchModelsMissing`]. The installed E5 model
    /// sits behind [`Self::embedder`] and loads on first use.
    /// Shared (`Arc`) so other stores, such as the statement store, embed with the same
    /// model without holding the engine lock during inference.
    embeddings: Option<Arc<dyn EmbeddingModel>>,
    /// The E5 model behind `embeddings` (empty for a test embedder); its slot
    /// lives as long as the engine, so the idle unloader keeps one handle.
    embedder: SharedEmbedder,
    /// `embeddings` is the E5 model in [`Self::embedder`] (not a test embedder).
    embedder_e5: bool,
    chunker: TextChunker,
    structure_chunker: StructureChunker,
    parser: DocumentParser,
    config: RAGConfig,
    /// The cross-encoder, shared with other rankers (web and paper results)
    /// through [`Self::reranker_handle`]; filled when models are attached.
    reranker: SharedReranker,
    /// The PP-OCRv6 model the parser reads scans with; filled when the models
    /// are attached and it is installed. Loaded on the first scanned page.
    ocr: SharedOcrModel,
    /// The OCR child process, when scans are read out of process
    /// ([`Self::use_ocr_worker`]); it reads with the model in `ocr`'s place.
    ocr_worker: Option<Arc<crate::processing::ocr::worker::OcrWorker>>,
    /// What OCR read from indexed scans (see [`crate::processing::ocr::cache`]),
    /// emptied of files as they leave the index.
    ocr_cache: Arc<crate::processing::ocr::cache::OcrCache>,
    /// PDFs indexed by the fast parser that have table-candidate pages are sent
    /// here, for the table model to refine in the background (see
    /// [`crate::table_refinement`]).
    refinement_queue: Option<tokio::sync::mpsc::UnboundedSender<std::path::PathBuf>>,
    /// Scans read without the scan reader are reported here (see
    /// [`ScanReaderRequest`]).
    scan_reader_queue: Option<tokio::sync::mpsc::UnboundedSender<ScanReaderRequest>>,
    /// Ranks library files by the citation graph for queries about papers, methods or
    /// citations: a third list fused into search (see [`crate::search::graph_fusion`]).
    source_ranker: Option<Arc<dyn crate::search::graph_fusion::SourceRanker>>,
    /// [`Self::stale_sources`] found every chunk in the current [`INDEX_FORMAT`].
    no_stale_chunks: Arc<std::sync::atomic::AtomicBool>,
    /// [`Self::rechunk_sources`] found every chunk of the current [`CHUNKER_VERSION`].
    no_rechunk_chunks: Arc<std::sync::atomic::AtomicBool>,
    /// Told which files left the index, after every removal (see
    /// [`LibraryRemovalObserver`]).
    removal_observer: Option<Arc<dyn LibraryRemovalObserver>>,
    /// Tests: the next listing of the index in [`Self::clear_all_data`] fails.
    #[cfg(test)]
    fail_source_listing: std::sync::atomic::AtomicBool,
    /// Tests: the next keyword-index removal in [`Self::delete_by_source`] fails.
    #[cfg(test)]
    fail_text_removal: std::sync::atomic::AtomicBool,
    /// Tests: the next [`Self::commit_file`] fails before it stores anything.
    #[cfg(test)]
    fail_next_store: std::sync::atomic::AtomicBool,
    /// Tests: removing replaced chunks fails while set.
    #[cfg(test)]
    fail_replaced_removal: std::sync::atomic::AtomicBool,
}

impl RAGEngine {
    /// Open the stores and, when the model files are present, attach the
    /// search models (located, not loaded: they load on first search or
    /// indexing). Without them (first run) the engine starts in a degraded
    /// state: stores, listing and deletion work; search and indexing return
    /// [`SearchModelsMissing`] until [`Self::attach_search_models`]. A model
    /// that is present but fails to load is logged when first used and then
    /// treated the same way, so a damaged model never prevents the app from
    /// starting.
    pub async fn new(config: RAGConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir).ok();

        let lance_path = config.data_dir.join("lance_data");
        let store = LanceStore::new(
            lance_path.to_str().unwrap_or("./lance_data"),
            config.embedding.dimension,
        )
        .await
        .context("Failed to initialize LanceDB store")?;

        let text_search = TextSearch::new(config.data_dir.to_str().unwrap_or("./data"))
            .context("Failed to initialize Tantivy search")?;

        let models = if config.embedding.use_e5 && SearchModels::available(&config) {
            match SearchModels::load(&config) {
                Ok(models) => Some(models),
                Err(e) => {
                    tracing::error!(
                        model_dir = %config.embedding.model_dir.display(),
                        error = %format!("{e:#}"),
                        "Search models failed to load; starting without search"
                    );
                    None
                }
            }
        } else {
            tracing::warn!(
                model_dir = %config.embedding.model_dir.display(),
                "Search models are not installed; starting without search"
            );
            None
        };

        let chunker = TextChunker::new(
            config.chunking.chunk_size,
            config.chunking.chunk_overlap,
            config.chunking.min_chunk_size,
        );

        let structure_chunker = StructureChunker::new(config.chunking.max_tokens);
        let ocr = crate::processing::ocr::shared_ocr_model();
        let ocr_cache = Arc::new(crate::processing::ocr::cache::OcrCache::new(
            config.data_dir.join(OCR_CACHE_DIR),
        ));
        let parser = DocumentParser::new()
            .with_ocr(ocr.clone(), config.ocr.clone())
            .with_ocr_cache(ocr_cache.clone());
        let mut engine = Self {
            store,
            text_search,
            embeddings: None,
            embedder: Arc::new(LazyModel::new("embedder")),
            embedder_e5: false,
            chunker,
            structure_chunker,
            parser,
            config,
            reranker: Arc::new(LazyModel::new("reranker")),
            ocr,
            ocr_worker: None,
            ocr_cache,
            refinement_queue: None,
            scan_reader_queue: None,
            source_ranker: None,
            no_stale_chunks: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            no_rechunk_chunks: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            removal_observer: None,
            #[cfg(test)]
            fail_source_listing: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_text_removal: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_next_store: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_replaced_removal: std::sync::atomic::AtomicBool::new(false),
        };
        if let Some(models) = models {
            if let Err(e) = engine.attach_search_models(models) {
                tracing::error!(error = %e, "Search models rejected; starting without search");
            }
        }

        // After schema migration the Tantivy index is empty but LanceDB still
        // has all the chunks.  Rebuild the text index so searches work immediately.
        if engine.text_search.is_empty() {
            let lance_count = engine.store.count().await.unwrap_or(0);
            if lance_count > 0 {
                tracing::info!(
                    lance_chunks = lance_count,
                    "Tantivy index empty — rebuilding from LanceDB"
                );
                if let Err(e) = engine.rebuild_text_index().await {
                    tracing::error!(error = %e, "Failed to rebuild Tantivy index from LanceDB");
                }
            }
        }
        // Chunks a commit replaced just before the app last closed.
        if let Err(e) = engine.settle_replaced().await {
            tracing::warn!(error = %format!("{e:#}"), "replaced chunks not removed yet");
        }

        Ok(engine)
    }

    /// The shared slot holding the cross-encoder once it is loaded. The
    /// handle stays valid for the engine's life; read it at use time, since
    /// models attached later fill the same slot.
    pub fn reranker_handle(&self) -> SharedReranker {
        self.reranker.clone()
    }

    /// The slot holding the OCR model once it is loaded (for idle unloading).
    /// Valid for the engine's life; models attached later fill the same slot.
    pub fn ocr_handle(&self) -> SharedOcrModel {
        self.ocr.clone()
    }

    /// Points the OCR slot at the installed model under the model directory,
    /// or empties it when the model is not installed. Returns whether it is
    /// installed. Called on attach and after an install.
    pub fn attach_ocr_model(&self) -> bool {
        let dir = SearchModels::locate_ocr(&self.config);
        self.set_ocr_dir(dir.clone());
        dir.is_some()
    }

    /// Points OCR at the model in `dir` (`None`: not installed): the
    /// in-process slot and, when there is one, the OCR child.
    fn set_ocr_dir(&self, dir: Option<std::path::PathBuf>) {
        if let Some(worker) = &self.ocr_worker {
            worker.set_model_dir(dir.clone());
        }
        match dir {
            Some(dir) => self
                .ocr
                .set_loader(move || crate::processing::ocr::PpOcr::load(&dir)),
            None => self.ocr.uninstall(),
        }
    }

    /// Reads scans and images from now on in an OCR child process started
    /// with `command` ([`crate::processing::ocr::worker`]) instead of in this
    /// process. Returns the worker, for idle stopping. Call before the engine
    /// is shared.
    pub fn use_ocr_worker(
        &mut self,
        command: crate::processing::ocr::worker::WorkerCommand,
    ) -> Arc<crate::processing::ocr::worker::OcrWorker> {
        let worker = Arc::new(crate::processing::ocr::worker::OcrWorker::new(command));
        if self.ocr.available() {
            worker.set_model_dir(SearchModels::locate_ocr(&self.config));
        }
        self.parser = std::mem::take(&mut self.parser).with_ocr_worker(worker.clone());
        self.ocr_worker = Some(worker.clone());
        worker
    }

    /// The slot holding the E5 model once it is loaded (for idle unloading and
    /// the install check). Valid for the engine's life; models attached later
    /// fill the same slot.
    pub fn embedder_handle(&self) -> SharedEmbedder {
        self.embedder.clone()
    }

    /// Whether the search models are attached (search and indexing work). The
    /// E5 model may not be in memory yet: it loads on first use. A model that
    /// failed to load counts as missing, so setup is offered again.
    pub fn has_search_models(&self) -> bool {
        self.embeddings.is_some() && (!self.embedder_e5 || self.embedder.available())
    }

    /// Attach models loaded with [`SearchModels::load`]. Fails when the
    /// embedding dimension differs from the vector store's.
    pub fn attach_search_models(&mut self, models: SearchModels) -> Result<()> {
        let dimension = models.dimension();
        if dimension != self.config.embedding.dimension {
            anyhow::bail!(
                "embedding model produces {dimension}-dimensional vectors but the index uses {}",
                self.config.embedding.dimension
            );
        }
        self.embeddings = Some(match models.embeddings {
            EmbedderSource::E5(config) => {
                self.embedder
                    .set_loader(move || E5Embeddings::new(config.clone()));
                self.embedder_e5 = true;
                Arc::new(LazyEmbeddings {
                    model: self.embedder.clone(),
                    dimension,
                })
            }
            EmbedderSource::Loaded(model) => {
                self.embedder.uninstall();
                self.embedder_e5 = false;
                model
            }
        });
        match models.reranker_dir {
            Some(dir) => self
                .reranker
                .set_loader(move || CrossEncoderReranker::new(&dir)),
            None => self.reranker.uninstall(),
        }
        self.set_ocr_dir(models.ocr_dir);
        tracing::info!(
            dimension,
            reranker = self.reranker.available(),
            ocr = self.ocr.available(),
            "Search models attached"
        );
        Ok(())
    }

    fn require_embeddings(&self) -> Result<&dyn EmbeddingModel> {
        self.embeddings
            .as_deref()
            .ok_or_else(|| SearchModelsMissing.into())
    }

    /// Ingest a document from raw content
    pub async fn add_document(
        &mut self,
        content: &str,
        _format: DocumentFormat,
        metadata: HashMap<String, String>,
        citation: Citation,
    ) -> Result<Vec<Uuid>> {
        self.require_embeddings()?;
        let title = metadata
            .get("title")
            .cloned()
            .unwrap_or_else(|| "Untitled".to_string());
        let source = metadata
            .get("file_path")
            .or_else(|| metadata.get("source"))
            .cloned()
            .unwrap_or_default();

        // Contextual chunking: prepend document-level context to each chunk
        // before embedding for better retrieval (Anthropic's contextual retrieval approach)
        let embedder = self.require_embeddings()?;
        let count = |text: &str| {
            embedder
                .count_tokens(text)
                .unwrap_or_else(|| crate::embeddings::estimate_tokens(text))
        };
        let chunks = chunk_in_context(&title, true, &count, &|context| {
            self.chunker
                .chunk_with_context(content, context, source_label(&source))
        });
        let prepared = prepare_chunks(embedder, chunks, title, source, &metadata, &citation)?;
        self.store_prepared(prepared).await
    }

    /// Ingest a document from a file path.
    ///
    /// The file is parsed, chunked and embedded *before* any previously indexed
    /// chunks for the same source are touched, so a file that fails to parse or
    /// embed leaves its existing index entries intact. The replacement is then
    /// stored as [`Self::commit_file`] stores it: the new chunks first, the old
    /// ones removed after, so the file never has no chunks in between.
    ///
    /// This parses and embeds while the caller holds the engine. Indexing that
    /// shares the engine prepares with [`Self::file_preparer`] instead and stores
    /// with [`Self::commit_file`].
    pub async fn add_document_from_file(
        &mut self,
        path: &Path,
        metadata: HashMap<String, String>,
    ) -> Result<Vec<Uuid>> {
        let file = self.file_preparer()?.prepare(path, metadata)?;
        self.commit_file(file).await
    }

    /// What prepares files apart from the engine. Fails with [`SearchModelsMissing`]
    /// until the search models are attached.
    pub fn file_preparer(&self) -> Result<FilePreparer> {
        let embeddings = self
            .embeddings
            .clone()
            .ok_or_else(|| anyhow::Error::from(SearchModelsMissing))?;
        Ok(FilePreparer {
            parser: self.parser.clone(),
            chunker: self.chunker.clone(),
            structure_chunker: self.structure_chunker.clone(),
            embeddings,
        })
    }

    /// Stores a prepared file, replacing its previous chunks, and queues it for the
    /// table model when it has table-candidate pages.
    ///
    /// The new chunks are stored first; only then are the previous ones
    /// removed (by id, in LanceDB and the keyword index), so a failure or a
    /// quit at any point never leaves the file without chunks. A failure
    /// while storing leaves the previous chunks as they were. The file and
    /// its previous chunk ids are written down before anything is stored
    /// ([`REPLACED_CHUNKS_FILE`]): a quit or a failure before the old chunks
    /// are gone leaves both copies until the next commit or the next start
    /// removes the old ones ([`Self::settle_replaced`]), or keeps them when
    /// nothing new was stored. Rows
    /// written before paths were canonicalized may carry another spelling of
    /// the same file; they are replaced too, so nothing duplicates.
    pub async fn commit_file(&mut self, file: PreparedFile) -> Result<Vec<Uuid>> {
        let store_started = std::time::Instant::now();
        if let Err(e) = self.settle_replaced().await {
            tracing::warn!(error = %format!("{e:#}"), "replaced chunks not removed yet");
        }
        let mut previous = self.chunk_ids_of(&file.source).await?;
        for legacy in legacy_source_spellings(&file.path, &file.source) {
            previous.extend(self.chunk_ids_of(&legacy).await?);
        }
        // Written down before anything is stored, so a quit or a failure at
        // any later point is finished by `settle_replaced`.
        let written = !previous.is_empty() && self.begin_replacement(&file.source, &previous);
        if !previous.is_empty() && !written {
            // Without the record a quit after storing would leave both copies
            // for good: keep the previous chunks and let the file be retried.
            anyhow::bail!(
                "could not record the replacement of {}'s chunks; its index entries are unchanged",
                file.source
            );
        }
        let ids = match self.store_document(file.document).await {
            Ok(ids) => ids,
            Err(e) => {
                // Nothing was stored: the previous chunks are the file's still.
                if written {
                    self.end_replacement(&file.source);
                }
                return Err(e);
            }
        };
        let kept: std::collections::HashSet<String> = ids.iter().map(Uuid::to_string).collect();
        previous.retain(|id| !kept.contains(id));
        let removed = if previous.is_empty() {
            true
        } else {
            match self.delete_chunk_ids(&previous).await {
                Ok(()) => true,
                Err(e) if written => {
                    tracing::warn!(
                        error = %format!("{e:#}"),
                        chunks = previous.len(),
                        "replaced chunks not removed yet; they are removed at the next commit or start"
                    );
                    false
                }
                Err(e) => return Err(e.context("the replaced chunks could not be removed")),
            }
        };
        if written && removed {
            self.end_replacement(&file.source);
        }
        tracing::info!(
            source = %file.source,
            chunks = ids.len(),
            parse_ms = file.parse_ms,
            chunk_ms = file.chunk_ms,
            embed_ms = file.embed_ms,
            store_ms = store_started.elapsed().as_millis(),
            "Indexed file"
        );
        if file.refine {
            if let Some(queue) = &self.refinement_queue {
                // A closed queue only means no refinement runs; the index is complete.
                let _ = queue.send(file.path);
            }
        }
        Ok(ids)
    }

    /// Reports every scan indexed from now on without the scan reader to
    /// `queue` ([`ScanReaderRequest`]).
    pub fn set_scan_reader_queue(
        &mut self,
        queue: tokio::sync::mpsc::UnboundedSender<ScanReaderRequest>,
    ) {
        self.scan_reader_queue = Some(queue);
    }

    /// Reports a scan read without the scan reader (see
    /// [`Self::set_scan_reader_queue`]); nothing happens without a queue.
    pub fn report_scan_reader_missing(&self, path: &Path, metadata: HashMap<String, String>) {
        if let Some(queue) = &self.scan_reader_queue {
            // A closed queue only means nobody offers the reader.
            let _ = queue.send(ScanReaderRequest {
                path: path.to_path_buf(),
                metadata,
            });
        }
    }

    /// The parser the indexer reads files with (its OCR included), for
    /// readers of indexed files that must see what the index saw.
    pub fn parser(&self) -> DocumentParser {
        self.parser.clone()
    }

    /// Sends every PDF indexed from now on that has table-candidate pages to `queue`.
    pub fn set_refinement_queue(
        &mut self,
        queue: tokio::sync::mpsc::UnboundedSender<std::path::PathBuf>,
    ) {
        self.refinement_queue = Some(queue);
    }

    /// Fuses the citation graph's ranking of library files into every search from now
    /// on (`None` removes it).
    pub fn set_source_ranker(
        &mut self,
        ranker: Option<Arc<dyn crate::search::graph_fusion::SourceRanker>>,
    ) {
        self.source_ranker = ranker;
    }

    /// Reports every file that leaves the index from now on to `observer` (`None`
    /// stops). Call [`Self::settle_forgotten`] after setting it, to finish what an
    /// earlier run could not.
    pub fn set_removal_observer(&mut self, observer: Option<Arc<dyn LibraryRemovalObserver>>) {
        self.removal_observer = observer;
    }

    /// Where the removals the observer has not confirmed are written down.
    fn pending_forgets_path(&self) -> std::path::PathBuf {
        self.config.data_dir.join(PENDING_FORGETS_FILE)
    }

    /// Writes down, before they are removed, the files about to leave the index (or
    /// [`FORGET_ALL`]), so a quit or a failure between the removal and the report cannot
    /// lose them: the next report (or [`Self::settle_forgotten`]) drops the ones that are
    /// still indexed. The file work runs on the blocking pool.
    async fn begin_forget(&self, entries: Vec<String>) {
        if self.removal_observer.is_none() || entries.is_empty() {
            return;
        }
        let path = self.pending_forgets_path();
        let written = tokio::task::spawn_blocking(move || -> Result<()> {
            let mut pending = load_pending_forgets(&path)?;
            let known = pending.len();
            pending.extend(entries);
            if pending.len() == known {
                return Ok(());
            }
            write_pending_forgets(&path, &pending)
        })
        .await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!(
                error = %format!("{e:#}"),
                "pending removals not written down; they are reported only while the app runs"
            ),
            Err(e) => tracing::error!(error = %e, "writing the pending removals failed"),
        }
    }

    /// The files of `sources` that still have chunks in the index. Reads only the
    /// files asked for, 500 at a time.
    async fn still_indexed(&self, sources: &[String]) -> Result<std::collections::HashSet<String>> {
        let mut indexed = std::collections::HashSet::new();
        for batch in sources.chunks(500) {
            let quoted: Vec<String> = batch
                .iter()
                .map(|s| format!("'{}'", s.replace('\'', "''")))
                .collect();
            let predicate = format!("source IN ({})", quoted.join(", "));
            for row in self.store.document_sources_where(Some(&predicate)).await? {
                indexed.insert(row.source);
            }
        }
        Ok(indexed)
    }

    /// Tells the removal observer that `removed` left the index, together with the
    /// files an earlier removal could not report. `removed_for_sure` says the removal
    /// of `removed` succeeded (nothing is listed to check it); otherwise, and for the
    /// recorded files, the index is asked which files still have chunks, and those are
    /// not reported (a file added back, or only partly removed, is still in the
    /// library; it is dropped from the record). Called whatever the removal's outcome
    /// was, so a removal that failed halfway still forgets what it did remove.
    ///
    /// A report that fails is logged and kept in [`PENDING_FORGETS_FILE`], and tried
    /// again by the next removal and by [`Self::settle_forgotten`]. The observer and the
    /// record run on the blocking pool; the engine lock is held meanwhile, because the
    /// index is read under it and the report must not outlive an answer the caller is
    /// still waiting for. Returns the files the observer confirmed.
    async fn forget_removed(&self, removed: &[String], removed_for_sure: bool) -> usize {
        let Some(observer) = self.removal_observer.clone() else {
            return 0;
        };
        let removed: Vec<String> = file_sources(removed.iter().cloned());
        let path = self.pending_forgets_path();
        let load_path = path.clone();
        let loaded = tokio::task::spawn_blocking(move || load_pending_forgets(&load_path)).await;
        let (recorded, can_record) = match loaded {
            Ok(Ok(recorded)) => (recorded, true),
            Ok(Err(e)) => {
                tracing::error!(error = %format!("{e:#}"), "pending removals unreadable");
                (Default::default(), false)
            }
            Err(e) => {
                tracing::error!(error = %e, "reading the pending removals failed");
                (Default::default(), false)
            }
        };
        if removed.is_empty() && recorded.is_empty() {
            return 0;
        }
        let wants_all = recorded.contains(FORGET_ALL);
        let mut to_check: Vec<String> = file_sources(recorded.iter().cloned());
        let mut gone = std::collections::BTreeSet::new();
        let mut pending = std::collections::BTreeSet::new();
        if removed_for_sure {
            gone.extend(removed.iter().cloned());
        } else {
            to_check.extend(removed.iter().cloned());
        }
        if !to_check.is_empty() {
            match self.still_indexed(&to_check).await {
                Ok(indexed) => gone.extend(to_check.into_iter().filter(|s| !indexed.contains(s))),
                Err(e) => {
                    tracing::error!(
                        error = %format!("{e:#}"),
                        "could not tell which removed files are back in the index"
                    );
                    pending.extend(to_check);
                }
            }
        }
        let mut forget_all = false;
        if wants_all {
            match self.store.count().await {
                Ok(0) => forget_all = true,
                Ok(_) => tracing::warn!(
                    "a reset was recorded but the index has files; they are not forgotten"
                ),
                Err(e) => {
                    tracing::error!(error = %format!("{e:#}"), "could not tell whether the index is empty");
                    pending.insert(FORGET_ALL.to_string());
                }
            }
        }
        let gone: Vec<String> = gone.into_iter().collect();
        let reported = tokio::task::spawn_blocking(move || {
            let mut confirmed = 0;
            if forget_all {
                if let Err(e) = observer.forget_all() {
                    tracing::error!(
                        error = %format!("{e:#}"),
                        "the evidence tables did not forget everything; will retry"
                    );
                    pending.insert(FORGET_ALL.to_string());
                }
            }
            if !gone.is_empty() {
                match observer.forget_sources(&gone) {
                    Ok(()) => confirmed = gone.len(),
                    Err(e) => {
                        tracing::error!(
                            files = gone.len(),
                            error = %format!("{e:#}"),
                            "removed files were not forgotten by the evidence tables; will retry"
                        );
                        pending.extend(gone);
                    }
                }
            }
            if can_record && pending != recorded {
                if let Err(e) = write_pending_forgets(&path, &pending) {
                    tracing::error!(
                        error = %format!("{e:#}"),
                        "pending removals not written down; they are retried only while the app runs"
                    );
                }
            }
            confirmed
        })
        .await;
        reported.unwrap_or_else(|e| {
            tracing::error!(error = %e, "reporting removed files failed; they stay recorded");
            0
        })
    }

    /// Reports again the removals the observer did not confirm (a failure or a quit
    /// came first). Run after [`Self::set_removal_observer`] at start. Returns the files
    /// the observer confirmed.
    pub async fn settle_forgotten(&self) -> usize {
        self.forget_removed(&[], false).await
    }

    /// The graph's ranks of library files for `query`, when it is about the graph.
    fn graph_ranks(&self, query: &str) -> Option<HashMap<String, usize>> {
        let files = self.source_ranker.as_ref()?.rank_sources(query)?;
        let ranks = crate::search::graph_fusion::rank_map(&files);
        tracing::info!(
            query,
            graph_files = ranks.len(),
            "Citation graph ranks fused into search"
        );
        (!ranks.is_empty()).then_some(ranks)
    }

    /// Replaces the indexed chunks of a file with those of `refined` (the file
    /// re-parsed with the table model), keeping the document-level metadata the
    /// file was indexed with. Nothing is written when the file changed since it was
    /// re-parsed or is no longer indexed.
    ///
    /// This embeds while the caller holds the engine; the refinement worker uses
    /// [`Self::refined_metadata`], [`FilePreparer::prepare_parsed`] and
    /// [`Self::commit_refined`] to embed without it.
    pub async fn apply_refined_tables(
        &mut self,
        refined: crate::table_refinement::RefinedTables,
    ) -> Result<crate::table_refinement::RefineOutcome> {
        let metadata = match self.refined_metadata(&refined).await? {
            Ok(metadata) => metadata,
            Err(outcome) => return Ok(outcome),
        };
        let file = self.file_preparer()?.prepare_parsed(
            &refined.path,
            refined.parsed,
            metadata,
            refined.parse_ms,
        )?;
        self.commit_refined(file, refined.stamp, refined.model_tables)
            .await
    }

    /// The document-level metadata a refinement of `refined.path` keeps, or why it is
    /// not applied (the file changed since it was re-parsed, or is no longer indexed).
    pub async fn refined_metadata(
        &self,
        refined: &crate::table_refinement::RefinedTables,
    ) -> Result<std::result::Result<HashMap<String, String>, crate::table_refinement::RefineOutcome>>
    {
        use crate::table_refinement::{document_metadata, FileStamp, RefineOutcome};
        self.require_embeddings()?;
        match FileStamp::of(&refined.path) {
            Ok(stamp) if stamp == refined.stamp => {}
            _ => return Ok(Err(RefineOutcome::FileChanged)),
        }
        let source = normalize_source_path(&refined.path);
        let predicate = format!("source = '{}'", source.replace('\'', "''"));
        let Some(existing) = self
            .store
            .list_chunks(Some(&predicate), 1)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Err(RefineOutcome::NotIndexed));
        };
        let stored: HashMap<String, String> =
            serde_json::from_str(&existing.metadata_json).unwrap_or_default();
        Ok(Ok(document_metadata(&stored, &existing.space_id)))
    }

    /// Stores a refinement prepared from a re-parse of the file at `stamp`, unless the
    /// file changed or was removed from the index meanwhile.
    pub async fn commit_refined(
        &mut self,
        mut file: PreparedFile,
        stamp: crate::table_refinement::FileStamp,
        model_tables: usize,
    ) -> Result<crate::table_refinement::RefineOutcome> {
        use crate::table_refinement::{FileStamp, RefineOutcome};
        match FileStamp::of(&file.path) {
            Ok(now) if now == stamp => {}
            _ => return Ok(RefineOutcome::FileChanged),
        }
        let predicate = format!("source = '{}'", file.source.replace('\'', "''"));
        if self
            .store
            .list_chunks(Some(&predicate), 1)
            .await?
            .is_empty()
        {
            return Ok(RefineOutcome::NotIndexed);
        }
        // A refinement is never queued for refinement again.
        file.refine = false;
        let ids = self.commit_file(file).await?;
        Ok(RefineOutcome::Replaced {
            chunks: ids.len(),
            model_tables,
        })
    }

    /// The ids of the chunks stored under `source`.
    async fn chunk_ids_of(&self, source: &str) -> Result<Vec<String>> {
        self.store.ids_where(&source_predicate(source)).await
    }

    /// Where the replacements in progress are written down until their old
    /// chunks are removed ([`Self::commit_file`]).
    fn replaced_chunks_path(&self) -> std::path::PathBuf {
        self.config.data_dir.join(REPLACED_CHUNKS_FILE)
    }

    /// The replacements written down and not finished yet.
    fn replaced_chunks(&self) -> Result<Vec<Replacement>> {
        match std::fs::read_to_string(self.replaced_chunks_path()) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes `replacements` as the replacements not finished yet (none: the
    /// record goes): to a temporary file, renamed over the previous record.
    fn write_replaced(&self, replacements: &[Replacement]) -> Result<()> {
        let path = self.replaced_chunks_path();
        if replacements.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            };
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut out = std::fs::File::create(&tmp)?;
            out.write_all(&serde_json::to_vec(replacements)?)?;
            // On disk before the rename, so a power loss can't leave an empty record.
            out.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    /// Writes down that `source` is about to be replaced and that `previous`
    /// are its chunks now, before anything is stored. Returns whether it was
    /// written.
    fn begin_replacement(&self, source: &str, previous: &[String]) -> bool {
        let mut pending = self.replaced_chunks().unwrap_or_else(|e| {
            tracing::warn!(error = %format!("{e:#}"), "replacements record unreadable");
            Vec::new()
        });
        pending.retain(|r| r.source != source);
        pending.push(Replacement {
            source: source.to_string(),
            previous_ids: previous.to_vec(),
        });
        match self.write_replaced(&pending) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "replacement not written down");
                false
            }
        }
    }

    /// The replacement of `source` is finished (its old chunks are gone) or
    /// was given up (nothing was stored): its record goes.
    fn end_replacement(&self, source: &str) {
        let ended = self.replaced_chunks().and_then(|mut pending| {
            let before = pending.len();
            pending.retain(|r| r.source != source);
            if pending.len() == before {
                return Ok(());
            }
            self.write_replaced(&pending)
        });
        if let Err(e) = ended {
            tracing::warn!(error = %format!("{e:#}"), "replacements record not updated");
        }
    }

    /// Removes the chunks `ids` from the keyword index and LanceDB. Ids that
    /// are gone already are no matter.
    async fn delete_chunk_ids(&mut self, ids: &[String]) -> Result<()> {
        #[cfg(test)]
        if self
            .fail_replaced_removal
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            anyhow::bail!("removing the replaced chunks failed (injected)");
        }
        for id in ids {
            self.text_search.delete_by_id(id)?;
        }
        self.text_search.commit()?;
        for batch in ids.chunks(500) {
            let quoted: Vec<String> = batch
                .iter()
                .map(|id| format!("'{}'", id.replace('\'', "''")))
                .collect();
            self.store
                .delete_where(&format!("id IN ({})", quoted.join(", ")))
                .await?;
        }
        Ok(())
    }

    /// Finishes the replacements written down and not finished (a quit or a
    /// failure came first). Where the source has chunks other than its
    /// previous ones, the new ones were stored: the previous ones are
    /// removed. Where it has none, nothing was stored: the previous ones are
    /// its chunks still, and the record is dropped. Runs at the start of
    /// every commit and when the engine opens. Returns the chunks removed.
    pub async fn settle_replaced(&mut self) -> Result<usize> {
        let pending = self.replaced_chunks()?;
        if pending.is_empty() {
            return Ok(0);
        }
        let mut removed = 0;
        for replacement in &pending {
            let previous: std::collections::HashSet<&String> =
                replacement.previous_ids.iter().collect();
            let stored = self.chunk_ids_of(&replacement.source).await?;
            if stored.iter().any(|id| !previous.contains(id)) {
                self.delete_chunk_ids(&replacement.previous_ids).await?;
                removed += replacement.previous_ids.len();
            }
        }
        self.write_replaced(&[])?;
        if removed > 0 {
            tracing::info!(chunks = removed, "replaced chunks removed");
        }
        Ok(removed)
    }

    /// Remove every stored chunk for `source` from both LanceDB and Tantivy.
    async fn remove_source_chunks(&mut self, source: &str) -> Result<()> {
        if let Err(e) = self.store.delete_by_source(source).await {
            tracing::warn!(
                source = source,
                error = %e,
                "Failed to delete previous chunks from LanceDB before re-indexing"
            );
        }
        self.text_search.delete_by_source(source)?;
        self.text_search.commit()?;
        Ok(())
    }

    /// [`Self::store_prepared`] for a commit (tests can make it fail).
    async fn store_document(&mut self, prepared: PreparedDocument) -> Result<Vec<Uuid>> {
        #[cfg(test)]
        if self
            .fail_next_store
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            anyhow::bail!("storing the chunks failed (injected)");
        }
        self.store_prepared(prepared).await
    }

    /// Write prepared records to LanceDB and Tantivy.
    async fn store_prepared(&mut self, prepared: PreparedDocument) -> Result<Vec<Uuid>> {
        if prepared.chunk_ids.is_empty() {
            return Ok(Vec::new());
        }

        self.store
            .upsert_chunks(prepared.records)
            .await
            .context("Failed to store chunks in LanceDB")?;
        self.text_search.index_chunks_batch(&prepared.fts_batch)?;
        self.text_search.commit()?;

        tracing::info!(
            "Ingested document '{}' ({} chunks) into space '{}'",
            prepared.title,
            prepared.chunk_ids.len(),
            prepared.space_id,
        );

        Ok(prepared.chunk_ids)
    }

    /// Search with hybrid vector + FTS fusion
    pub async fn search(&self, query: &str, k: usize) -> Result<Vec<SimpleSearchResult>> {
        let results = self.search_comprehensive(query, k, None).await?;

        Ok(results
            .into_iter()
            .map(|r| {
                let doc_id = r
                    .metadata
                    .get("doc_id")
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .unwrap_or_default();
                let chunk_id = r
                    .metadata
                    .get("chunk_index")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                let title = r.citation.title.clone();
                let source = r.citation.source.clone();
                let heading = r
                    .metadata
                    .get("heading")
                    .filter(|h| !h.trim().is_empty())
                    .cloned();
                SimpleSearchResult {
                    id: r.id,
                    score: r.score,
                    text: r.snippet.clone(),
                    metadata: r.metadata,
                    title,
                    source,
                    heading,
                    citation: Some(r.citation),
                    doc_id,
                    chunk_id,
                }
            })
            .collect())
    }

    /// Full search with filters, reranking, and source tracking.
    /// Automatically decomposes multi-part queries into sub-queries for parallel retrieval.
    pub async fn search_comprehensive(
        &self,
        query: &str,
        k: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<ComprehensiveResult>> {
        // Checked before decomposition: sub-query failures are swallowed
        // there, which would turn "not installed" into "no results".
        self.require_embeddings()?;
        // The graph is asked about the whole query, also when it is decomposed.
        let graph_ranks = self.graph_ranks(query);
        // Likewise the intent: a sub-query may lose the words that asked for references.
        let keep_references = seeks_references(query);
        // Decompose complex queries into independent sub-queries
        let decomposed = crate::rag::query_decomposer::decompose_query(query);

        if decomposed.sub_queries.len() > 1 {
            tracing::debug!(
                original = query,
                sub_queries = ?decomposed.sub_queries,
                strategy = ?decomposed.strategy,
                "Query decomposed"
            );

            // Search each sub-query independently
            let mut result_sets = Vec::new();
            for sub_query in &decomposed.sub_queries {
                match self
                    .search_single_query(
                        sub_query,
                        k,
                        filter.clone(),
                        graph_ranks.as_ref(),
                        keep_references,
                    )
                    .await
                {
                    Ok(results) => result_sets.push(results),
                    Err(e) => {
                        tracing::warn!(sub_query = sub_query, error = %e, "Sub-query search failed");
                    }
                }
            }

            if result_sets.is_empty() {
                return Ok(Vec::new());
            }

            // Merge with round-robin interleaving and deduplication
            let mut merged = crate::rag::query_decomposer::merge_results(result_sets, k);

            // Expand with neighbors on the merged set
            self.expand_with_neighbors(&mut merged, 1).await;

            return Ok(merged);
        }

        let mut results = self
            .search_single_query(query, k, filter, graph_ranks.as_ref(), keep_references)
            .await?;
        self.expand_with_neighbors(&mut results, 1).await;
        Ok(results)
    }

    /// Execute a single search query through the full pipeline.
    async fn search_single_query(
        &self,
        query: &str,
        k: usize,
        filter: Option<MetadataFilter>,
        graph_ranks: Option<&HashMap<String, usize>>,
        keep_references: bool,
    ) -> Result<Vec<ComprehensiveResult>> {
        // Use same candidate count for both vector and FTS for balanced fusion
        let candidate_count = k * self.config.search.candidate_multiplier;

        // Generate query embedding
        let query_embedding = self.require_embeddings()?.embed_query(query)?;

        // Build filter predicate for LanceDB
        let lance_filter = filter.as_ref().and_then(|f| f.to_lance_predicate());

        // Extract source filter for FTS consistency
        let source_filter = filter.as_ref().and_then(|f| f.source_path.as_deref());

        // Vector search via LanceDB
        let vector_hits = self
            .store
            .vector_search(&query_embedding, candidate_count, lance_filter.as_deref())
            .await?;

        let vector_results: Vec<(String, f32)> = vector_hits
            .iter()
            .map(|h| (h.id.clone(), h.score))
            .collect();

        // Full-text search via Tantivy — use SAME candidate count for balanced fusion
        let fts_results =
            self.text_search
                .search_filtered(query, candidate_count, source_filter)?;

        // Log source diversity at each stage for diagnostics
        let vector_sources: std::collections::HashSet<&str> =
            vector_hits.iter().map(|h| h.source.as_str()).collect();
        tracing::info!(
            query = query,
            candidate_count = candidate_count,
            vector_hits = vector_hits.len(),
            vector_unique_sources = vector_sources.len(),
            fts_hits = fts_results.len(),
            vector_sources = ?vector_sources,
            "Hybrid search candidates"
        );

        // Score-aware Reciprocal Rank Fusion — preserves original quality signals
        let fused = score_aware_rrf(
            vector_results,
            fts_results,
            self.config.search.rrf_k,
            candidate_count, // Get more candidates for reranking
            self.config.search.score_weight,
        );

        tracing::info!(
            fused_count = fused.len(),
            threshold = self.config.search.min_score_threshold,
            "RRF fusion complete"
        );

        // Build hit_map from vector results for fast lookup
        let hit_map: HashMap<String, &crate::storage::SearchHit> =
            vector_hits.iter().map(|h| (h.id.clone(), h)).collect();

        // Collect IDs that are FTS-only (not in vector results)
        let fts_only_ids: Vec<String> = fused
            .iter()
            .filter(|(id, _, _)| !hit_map.contains_key(id))
            .map(|(id, _, _)| id.clone())
            .collect();

        // Fetch full data for FTS-only results from LanceDB
        let fts_only_hits = if !fts_only_ids.is_empty() {
            self.store.get_by_ids(&fts_only_ids).await?
        } else {
            Vec::new()
        };
        let fts_only_map: HashMap<String, &crate::storage::SearchHit> =
            fts_only_hits.iter().map(|h| (h.id.clone(), h)).collect();

        // Log top fused scores for diagnostics
        if let Some((top_id, top_score, _)) = fused.first() {
            tracing::info!(
                top_fused_id = top_id,
                top_fused_score = top_score,
                hit_map_size = hit_map.len(),
                fts_only_map_size = fts_only_map.len(),
                "Fused score diagnostics"
            );
        }

        // Build result objects
        let mut results = Vec::with_capacity(fused.len());

        for (id, score, source) in &fused {
            let source_label = match source {
                HybridSource::Vector => "lance",
                HybridSource::TextSearch => "tantivy",
                HybridSource::Both => "hybrid",
            };

            // Look up in vector hits first, then FTS-only hits
            let hit = hit_map.get(id).or_else(|| fts_only_map.get(id));

            if let Some(hit) = hit {
                let metadata: HashMap<String, String> =
                    serde_json::from_str(&hit.metadata_json).unwrap_or_default();
                let mut citation: Citation =
                    serde_json::from_str(&hit.citation_json).unwrap_or_default();

                let mut full_metadata = metadata;
                full_metadata.insert("doc_id".to_string(), hit.doc_id.clone());
                full_metadata.insert("chunk_index".to_string(), hit.chunk_index.to_string());
                full_metadata.insert("source_file".to_string(), hit.source.clone());
                full_metadata.insert("space_id".to_string(), hit.space_id.clone());
                if !hit.heading.trim().is_empty() {
                    full_metadata
                        .entry("heading".to_string())
                        .or_insert_with(|| hit.heading.clone());
                }
                if citation.page_numbers.is_none() {
                    citation.page_numbers = page_numbers_from_metadata(&full_metadata);
                }

                results.push(ComprehensiveResult {
                    id: Uuid::parse_str(&hit.id).unwrap_or_default(),
                    score: *score,
                    metadata: full_metadata,
                    citation,
                    snippet: hit.text.clone(),
                    source_index: source_label.to_string(),
                });
            }
            // Skip results where we can't find full data (shouldn't happen now)
        }

        // Every result honours the filter's source limits: keyword-only candidates were
        // fetched by id without the vector store's predicate.
        if let Some(filter) = &filter {
            let before = results.len();
            results.retain(|r| {
                filter.admits(
                    r.metadata.get("space_id").map_or("", String::as_str),
                    r.metadata.get("source_file").map_or("", String::as_str),
                )
            });
            if results.len() < before {
                tracing::debug!(
                    dropped = before - results.len(),
                    "results outside the search's sources removed"
                );
            }
        }

        // Bibliography entries are short and dense in names and topics, so they score
        // well on almost any query and pushed real content down. They answer only
        // queries about citations; dropped before the threshold, reranking and the
        // cut to `k`, so the k results returned are content.
        if !keep_references {
            results.retain(|r| !is_reference_entry(&r.metadata));
        }

        // Log source diversity of built results
        {
            let built_sources: std::collections::HashSet<&str> = results
                .iter()
                .filter_map(|r| r.metadata.get("source_file").map(|s| s.as_str()))
                .collect();
            tracing::info!(
                built_results = results.len(),
                unique_sources = built_sources.len(),
                sources = ?built_sources,
                "Results built from fused candidates"
            );
        }

        // The citation graph as a third RRF list, before the threshold decides.
        if let Some(ranks) = graph_ranks {
            let boosted = crate::search::graph_fusion::boost_scores(
                &mut results,
                ranks,
                self.config.search.rrf_k,
            );
            tracing::info!(boosted, "Graph list fused");
        }

        // Filter by minimum score threshold
        let threshold = self.config.search.min_score_threshold;
        let pre_filter_count = results.len();
        results.retain(|r| r.score >= threshold);
        tracing::info!(
            pre_filter = pre_filter_count,
            post_filter = results.len(),
            threshold = threshold,
            top_score = results.first().map(|r| r.score).unwrap_or(0.0),
            "Score threshold filter"
        );

        // Deduplicate near-identical chunks (from overlapping windows)
        Self::deduplicate_results(&mut results, 0.75);

        // Apply cross-encoder reranking if available (before MMR so diversity uses final scores)
        let reranker = self.reranker.get();
        if let Some(reranker) = &reranker {
            if results.len() > 1 {
                // The cross-encoder reads each passage with the context it was
                // embedded with: which document, its opening, which section.
                let candidates: Vec<(String, String, String)> = results
                    .iter()
                    .map(|r| (r.id.to_string(), rerank_context(r), r.snippet.clone()))
                    .collect();

                match reranker.rerank_in_context(query, &candidates, candidates.len()) {
                    Ok(reranked) => {
                        let logits: HashMap<String, f32> = reranked.into_iter().collect();
                        Self::apply_rerank_scores(&mut results, &logits);
                        // The cross-encoder replaced every score; fuse the graph's order
                        // again so its vote survives (scores keep their scale).
                        if let Some(ranks) = graph_ranks {
                            crate::search::graph_fusion::fuse_ranks(
                                &mut results,
                                ranks,
                                self.config.search.rrf_k,
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Reranking failed, using fusion scores: {}", e);
                    }
                }
            }
        }

        // MMR diversity: penalize repeated source files to spread results across documents.
        // Each additional chunk from the same source gets score *= lambda^count.
        // This naturally balances depth vs diversity without an artificial hard cap.
        Self::apply_mmr_diversity(&mut results, 0.5);
        Self::demote_reference_entries(&mut results);

        // Log final diversity before truncation
        {
            let final_sources: std::collections::HashSet<&str> = results
                .iter()
                .filter_map(|r| r.metadata.get("source_file").map(|s| s.as_str()))
                .collect();
            tracing::info!(
                post_mmr_results = results.len(),
                unique_sources = final_sources.len(),
                scores = ?results.iter().map(|r| format!("{:.3}", r.score)).collect::<Vec<_>>(),
                sources = ?results.iter().filter_map(|r| r.metadata.get("source_file").map(|s| s.as_str())).collect::<Vec<_>>(),
                "Post-MMR+cap diversity"
            );
        }

        // Final truncation to requested k
        results.truncate(k);

        Ok(results)
    }

    /// Expand top-k results with neighboring chunks from the same document.
    /// For each result, fetches ±window adjacent chunks by chunk_index and
    /// concatenates them in reading order (prev + current + next), recording
    /// where each part lies under [`SNIPPET_PARTS_KEY`] so a reader that cuts
    /// the snippet keeps the chunk that matched. Structure chunks are left as
    /// they are (see [`wants_neighbor_expansion`]).
    async fn expand_with_neighbors(&self, results: &mut [ComprehensiveResult], window: u32) {
        for result in results.iter_mut() {
            if !wants_neighbor_expansion(&result.metadata) {
                continue;
            }
            let doc_id = match result.metadata.get("doc_id") {
                Some(id) if !id.is_empty() => id.clone(),
                _ => continue,
            };
            let chunk_index: u32 = result
                .metadata
                .get("chunk_index")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            match self.store.get_neighbors(&doc_id, chunk_index, window).await {
                Ok(neighbors) if !neighbors.is_empty() => {
                    let (expanded, parts) =
                        join_neighbors(&result.snippet, chunk_index, &neighbors);
                    // Without its parts a cut would keep the snippet's start, the
                    // chunk before the match: the result stays unexpanded then.
                    match serde_json::to_string(&parts) {
                        Ok(json) => {
                            result.metadata.insert(SNIPPET_PARTS_KEY.to_string(), json);
                            result.snippet = expanded;
                        }
                        Err(e) => {
                            tracing::debug!("Neighbor expansion failed for doc {}: {}", doc_id, e);
                        }
                    }
                }
                Ok(_) => {} // No neighbors found
                Err(e) => {
                    tracing::debug!("Neighbor expansion failed for doc {}: {}", doc_id, e);
                }
            }
        }
    }

    /// Delete all chunks belonging to a specific document (by doc_id).
    /// Removes from both LanceDB and Tantivy.
    pub async fn delete_by_doc_id(&mut self, doc_id: &str) -> Result<usize> {
        self.forget_cached_scans(|row| row.doc_id == doc_id).await;
        // Get all chunk IDs for this doc so we can remove from Tantivy
        let predicate = format!("doc_id = '{}'", doc_id.replace('\'', "''"));
        let chunks = self.store.list_chunks(Some(&predicate), 100_000).await?;
        let sources = file_sources(
            self.store
                .document_sources_where(Some(&predicate))
                .await?
                .into_iter()
                .map(|row| row.source),
        );
        self.begin_forget(sources.clone()).await;

        let outcome: Result<usize> = async {
            for chunk in &chunks {
                let _ = self.text_search.delete_by_id(&chunk.id);
            }
            if !chunks.is_empty() {
                self.text_search.commit()?;
            }
            let deleted = self.store.delete_by_doc_id(doc_id).await?;
            tracing::info!(doc_id = %doc_id, deleted = deleted, "Deleted document by doc_id");
            Ok(deleted)
        }
        .await;
        self.forget_removed(&sources, false).await;
        outcome
    }

    /// Delete all documents from a specific source/folder.
    /// The source path is normalized the same way as during indexing so that
    /// Windows backslash / mixed-case paths always match.
    pub async fn delete_by_source(&mut self, source: &str) -> Result<usize> {
        let normalized = normalize_source_path(Path::new(source));
        // A calendar item or a note is not a paper: nothing to forget, and no cost.
        let tracked = is_file_source(source) && is_file_source(&normalized);
        if tracked {
            self.begin_forget(vec![normalized.clone()]).await;
        }
        let outcome: Result<usize> = async {
            let deleted = self.store.delete_by_source(&normalized).await?;
            #[cfg(test)]
            if self
                .fail_text_removal
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                anyhow::bail!("removing from the keyword index failed (injected)");
            }
            self.text_search.delete_by_source(&normalized)?;
            self.text_search.commit()?;
            self.ocr_cache.remove(&normalized);
            Ok(deleted)
        }
        .await;
        if tracked {
            // After a successful removal the file is gone by construction; after a
            // failed one the index is asked what is left of it.
            self.forget_removed(&[normalized], outcome.is_ok()).await;
        }
        outcome
    }

    /// Removes the OCR cache entries of the indexed files `leaving` selects
    /// (called before they are deleted from the index).
    async fn forget_cached_scans(
        &self,
        leaving: impl Fn(&crate::storage::DocumentSourceRow) -> bool,
    ) {
        match self.store.document_sources().await {
            Ok(rows) => {
                for row in rows.iter().filter(|row| leaving(row)) {
                    self.ocr_cache.remove(&row.source);
                }
            }
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "OCR cache not cleaned"),
        }
    }

    /// Deletes the Library source `space_id` from the index: every chunk it
    /// owns and, under `path` (the source's folder, or its file), the chunks
    /// that no source owns because they were indexed before chunks recorded
    /// their source (an empty `space_id`), except those under `keep` (the
    /// folders of the other Library sources). Files added in a chat
    /// (`uploaded_at` in their metadata) stay: older versions gave them the
    /// id of the Library source selected at the time, and those are handed
    /// to [`CHAT_UPLOADS_MAIN`] instead of deleted (they were added before
    /// workspaces owned their uploads, and nothing records the chat). The
    /// chunks of every other source stay too: a folder whose path merely
    /// starts with `path` ("Papers 2024" for "Papers"), or a source listed
    /// inside it.
    ///
    /// Everything is read before anything changes, so a failure while
    /// reading leaves the index as it was. Returns the chunks deleted.
    pub async fn delete_library_source(
        &mut self,
        space_id: &str,
        path: Option<&str>,
        keep: &[String],
    ) -> Result<usize> {
        if space_id.trim().is_empty() {
            anyhow::bail!("a Library source is removed by its id, and none was given");
        }
        if is_chat_uploads(space_id) {
            anyhow::bail!("files added in chats are not a Library source");
        }
        let predicate = format!("space_id = '{}'", space_id.replace('\'', "''"));
        let uploads: std::collections::BTreeSet<String> = self
            .store
            .list_chunks(Some(&predicate), 1_000_000)
            .await?
            .into_iter()
            .filter(|chunk| added_in_chat(&chunk.metadata_json))
            .map(|chunk| chunk.source)
            .collect();
        let unowned = match path.map(str::trim).filter(|p| !p.is_empty()) {
            Some(path) => self.unowned_under(Path::new(path), keep, space_id).await?,
            None => Vec::new(),
        };
        for source in &uploads {
            self.move_source_to_space(source, space_id, CHAT_UPLOADS_MAIN)
                .await?;
        }
        let mut deleted = self.delete_by_space_id(space_id).await?;
        deleted += self.delete_chunks(&unowned).await?;
        Ok(deleted)
    }

    /// The chunks no source owns of the file `path` or of the files under
    /// the folder `path` ([`lies_under`]), except files added in a chat,
    /// files under any of `keep`, and files a source other than `removing`
    /// has chunks of (a listed single-file source, say, whose path is known
    /// only from its chunks). Matched by plain string comparison: no `LIKE`
    /// pattern is involved.
    async fn unowned_under(
        &self,
        path: &Path,
        keep: &[String],
        removing: &str,
    ) -> Result<Vec<crate::storage::SearchHit>> {
        let spellings = source_spellings(path);
        let kept: Vec<Vec<String>> = keep
            .iter()
            .map(|folder| source_spellings(Path::new(folder)))
            .collect();
        let owned_elsewhere: std::collections::HashSet<String> = self
            .store
            .document_sources()
            .await?
            .into_iter()
            .filter(|row| !row.space_id.is_empty() && row.space_id != removing)
            .map(|row| row.source)
            .collect();
        Ok(self
            .store
            .list_chunks(Some("space_id = '' OR space_id IS NULL"), 1_000_000)
            .await?
            .into_iter()
            .filter(|chunk| {
                lies_under(&chunk.source, &spellings)
                    && !kept.iter().any(|k| lies_under(&chunk.source, k))
                    && !owned_elsewhere.contains(&chunk.source)
                    && !added_in_chat(&chunk.metadata_json)
            })
            .collect())
    }

    /// Deletes `chunks` by id from LanceDB and the keyword index, with their
    /// files' OCR cache entries. Returns the chunks deleted.
    async fn delete_chunks(&mut self, chunks: &[crate::storage::SearchHit]) -> Result<usize> {
        if chunks.is_empty() {
            return Ok(0);
        }
        let sources: std::collections::BTreeSet<String> =
            chunks.iter().map(|chunk| chunk.source.clone()).collect();
        let tracked = file_sources(sources.iter().cloned());
        self.begin_forget(tracked.clone()).await;
        let outcome: Result<usize> = async {
            for source in &sources {
                self.ocr_cache.remove(source);
            }
            for chunk in chunks {
                self.text_search.delete_by_id(&chunk.id)?;
            }
            self.text_search.commit()?;
            let mut deleted = 0;
            for batch in chunks.chunks(500) {
                let ids: Vec<String> = batch
                    .iter()
                    .map(|chunk| format!("'{}'", chunk.id.replace('\'', "''")))
                    .collect();
                deleted += self
                    .store
                    .delete_where(&format!("id IN ({})", ids.join(", ")))
                    .await?;
            }
            tracing::info!(
                deleted,
                "Deleted chunks without a source under a removed Library source"
            );
            Ok(deleted)
        }
        .await;
        self.forget_removed(&tracked, false).await;
        outcome
    }

    /// Delete all chunks belonging to a specific space.
    /// Removes from both the vector store (LanceDB) and the text search index (Tantivy).
    pub async fn delete_by_space_id(&mut self, space_id: &str) -> Result<usize> {
        self.forget_cached_scans(|row| row.space_id == space_id)
            .await;
        // First, get all chunk IDs for this space so we can remove them from Tantivy
        let predicate = format!("space_id = '{}'", space_id.replace('\'', "''"));
        let chunks = self.store.list_chunks(Some(&predicate), 1_000_000).await?;
        // The files to forget come from the space's document listing, which has no cap:
        // the chunk listing above stops at a million chunks, the deletion below does not.
        let sources = file_sources(
            self.store
                .document_sources_where(Some(&predicate))
                .await?
                .into_iter()
                .map(|row| row.source),
        );
        self.begin_forget(sources.clone()).await;

        let outcome: Result<usize> = async {
            // Delete each chunk from the Tantivy text search index by ID
            for chunk in &chunks {
                let _ = self.text_search.delete_by_id(&chunk.id);
            }
            self.text_search.commit()?;

            // Delete from LanceDB by space_id
            let deleted = self.store.delete_by_space_id(space_id).await?;

            tracing::info!(
                space_id = %space_id,
                deleted_chunks = deleted,
                "Deleted space data from vector store and text index"
            );
            Ok(deleted)
        }
        .await;
        // A file with chunks in another space is still in the library; the index says so.
        self.forget_removed(&sources, false).await;
        outcome
    }

    /// Gives the indexed file `source` of the space `from` to the space `to`
    /// without indexing it again: its chunks keep their ids, text, vectors
    /// and keyword entries, and only their owner changes, in the `space_id`
    /// column and in their metadata (so [`Self::document_metadata`] and
    /// [`Self::upgrade_in_place`] see the same owner). All chunks move in one
    /// LanceDB update, or none do. The keyword index stores no space: its
    /// hits are scoped by the LanceDB rows they are read from. Returns the
    /// chunks moved (0 when `source` has none in `from`).
    pub async fn move_source_to_space(
        &mut self,
        source: &str,
        from: &str,
        to: &str,
    ) -> Result<u64> {
        let moved = self.store.move_source_to_space(source, from, to).await?;
        tracing::info!(
            source,
            from,
            to,
            chunks = moved,
            "indexed file given to another source"
        );
        Ok(moved)
    }

    /// Gives every file of the space `from` to the space `to`
    /// ([`Self::move_source_to_space`], one file at a time). Returns the
    /// chunks moved.
    pub async fn move_space(&mut self, from: &str, to: &str) -> Result<u64> {
        let sources: std::collections::BTreeSet<String> = self
            .store
            .document_sources()
            .await?
            .into_iter()
            .filter(|row| row.space_id == from)
            .map(|row| row.source)
            .collect();
        let mut moved = 0;
        for source in &sources {
            moved += self.move_source_to_space(source, from, to).await?;
        }
        Ok(moved)
    }

    /// Gives every file added in a chat that is not owned where it was added
    /// to the main Ask ([`CHAT_UPLOADS_MAIN`]), once per run, before a folder
    /// sync could take it for one of its own: files older versions filed
    /// under the Library source selected at the time (or under no source),
    /// those an earlier build filed under a single `chat-uploads` owner, and,
    /// when `workspaces` (the workspaces that exist) is known, those of a
    /// workspace that was deleted. Nothing records the chat an older upload
    /// came from, so the main Ask is where it can still be found. Only the
    /// chat upload records `uploaded_at`. Returns the chunks moved.
    pub async fn hand_over_chat_uploads(
        &mut self,
        workspaces: Option<&std::collections::HashSet<String>>,
    ) -> Result<u64> {
        // `_` matches itself too: the filter only narrows what is read; the
        // metadata is checked exactly below.
        let candidates = self
            .store
            .list_chunks(Some("metadata_json LIKE '%uploaded_at%'"), 1_000_000)
            .await?;
        let strays: std::collections::BTreeSet<(String, String)> = candidates
            .into_iter()
            .filter(|chunk| added_in_chat(&chunk.metadata_json))
            .filter(
                |chunk| match chunk.space_id.strip_prefix(CHAT_UPLOADS_PREFIX) {
                    Some("main") => false,
                    Some(workspace) => workspaces.is_some_and(|live| !live.contains(workspace)),
                    None => true,
                },
            )
            .map(|chunk| (chunk.source, chunk.space_id))
            .collect();
        let mut moved = 0;
        for (source, owner) in &strays {
            moved += self
                .move_source_to_space(source, owner, CHAT_UPLOADS_MAIN)
                .await?;
        }
        Ok(moved)
    }

    /// Compacts the chunk store: an update rewrites the rows it changes, so
    /// after [`Self::move_source_to_space`] has moved files the old rows are
    /// removed from disk here, once for all of them.
    pub async fn compact_chunks(&mut self) -> Result<()> {
        self.store.compact().await
    }

    /// Clear all data
    pub async fn clear_all_data(&mut self) -> Result<()> {
        // A reset must work on an index that cannot be read: when the files cannot be
        // listed, the clear goes on and the tables are told to forget everything.
        let listed: Result<Vec<String>> = async {
            #[cfg(test)]
            if self
                .fail_source_listing
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                anyhow::bail!("listing the index failed (injected)");
            }
            Ok(self
                .store
                .document_sources()
                .await?
                .into_iter()
                .map(|row| row.source)
                .collect())
        }
        .await;
        let (sources, entries) = match listed {
            Ok(sources) => {
                let sources = file_sources(sources);
                (sources.clone(), sources)
            }
            Err(e) => {
                tracing::error!(
                    error = %format!("{e:#}"),
                    "the index could not be listed before it was cleared"
                );
                (Vec::new(), vec![FORGET_ALL.to_string()])
            }
        };
        self.begin_forget(entries).await;
        let outcome: Result<()> = async {
            self.store.clear().await?;
            self.text_search.clear()?;
            self.ocr_cache.clear();
            Ok(())
        }
        .await;
        self.forget_removed(&sources, outcome.is_ok()).await;
        outcome
    }

    /// Get statistics about the current state
    pub async fn get_statistics(&self) -> Result<HashMap<String, String>> {
        let mut stats = HashMap::new();
        let chunk_count = self.store.count().await?;
        let doc_count = self.store.count_documents().await.unwrap_or(0);
        let fts_count = self.text_search.count()?;

        stats.insert("total_chunks".to_string(), chunk_count.to_string());
        stats.insert("total_documents".to_string(), doc_count.to_string());
        stats.insert("fts_indexed".to_string(), fts_count.to_string());
        stats.insert(
            "embedding_dimension".to_string(),
            self.config.embedding.dimension.to_string(),
        );
        stats.insert(
            "search_models_loaded".to_string(),
            self.has_search_models().to_string(),
        );
        stats.insert(
            "data_dir".to_string(),
            self.config.data_dir.display().to_string(),
        );

        Ok(stats)
    }

    /// Whether at least one indexed chunk was produced from `path`.
    ///
    /// The path is compared after the same normalization applied at ingest
    /// time (forward slashes; lower-cased on Windows), and also verbatim for
    /// documents added through `add_document` with a caller-supplied
    /// `file_path`. Used to gate file access: only files the user indexed may
    /// be read back for display.
    pub async fn is_indexed_source(&self, path: &Path) -> Result<bool> {
        let normalized = normalize_source_path(path);
        let verbatim = path.display().to_string();
        let mut candidates = vec![normalized];
        if !candidates.contains(&verbatim) {
            candidates.push(verbatim);
        }
        for candidate in candidates {
            let predicate = format!("source = '{}'", candidate.replace('\'', "''"));
            if !self
                .store
                .list_chunks(Some(&predicate), 1)
                .await?
                .is_empty()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Count distinct documents in the index
    pub async fn count_documents(&self) -> Result<usize> {
        self.store.count_documents().await
    }

    /// Get document metadata for corpus stats: (doc_id, title, source)
    pub async fn get_document_info(&self) -> Result<Vec<(String, String, String)>> {
        self.store.get_document_info().await
    }

    /// How each document's file is named in the index, by `doc_id`: its
    /// stored [`DISPLAY_PATH_KEY`] and its `filename`, read from its first
    /// chunk (other metadata is skipped, not decoded). Documents indexed
    /// before the display path was stored have none; their spelling is
    /// resolved where it is shown ([`crate::path_case::PathSpellings`]).
    pub async fn document_names(&self) -> Result<HashMap<String, DocumentNames>> {
        Ok(self
            .store
            .first_chunk_metadata()
            .await?
            .into_iter()
            .filter_map(|(doc_id, json)| {
                let names: DocumentNames = serde_json::from_str(&json).ok()?;
                Some((doc_id, names))
            })
            .collect())
    }

    /// The indexed documents with chunks of an older [`INDEX_FORMAT`] than
    /// this build writes: they were embedded and keyword-indexed with another
    /// context than the chunks indexed now, so they rank on other terms. Each
    /// is indexed again by whatever indexed it (folder sync, the Library's
    /// file sources, the calendar), which replaces its chunks.
    pub async fn stale_sources(&self) -> Result<Vec<crate::storage::DocumentSourceRow>> {
        // Nothing writes an older format, so once none is left none comes
        // back: the scan over every chunk's metadata is skipped from then on.
        if self
            .no_stale_chunks
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(Vec::new());
        }
        let stale = self
            .store
            .document_sources_where(Some(&current_format_excluded()))
            .await?;
        if stale.is_empty() {
            self.no_stale_chunks
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(stale)
    }

    /// The indexed documents with chunks of an older [`CHUNKER_VERSION`]
    /// than this build makes: their text was read or cut another way, so
    /// each is read and chunked again in full by whatever indexed it (folder
    /// sync, the Library's file sources, the calendar). An upgrade in place
    /// cannot help them ([`Self::upgrade_in_place`]).
    pub async fn rechunk_sources(&self) -> Result<Vec<crate::storage::DocumentSourceRow>> {
        // Nothing writes an older version, so once none is left none comes back.
        if self
            .no_rechunk_chunks
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(Vec::new());
        }
        let outdated = self
            .store
            .document_sources_where(Some(&current_chunker_excluded()))
            .await?;
        if outdated.is_empty() {
            self.no_rechunk_chunks
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(outdated)
    }

    /// The indexed documents to index again: those of [`Self::stale_sources`]
    /// and of [`Self::rechunk_sources`], each once.
    pub async fn outdated_sources(&self) -> Result<Vec<crate::storage::DocumentSourceRow>> {
        let mut rows = self.stale_sources().await?;
        let mut seen: std::collections::HashSet<String> =
            rows.iter().map(|row| row.doc_id.clone()).collect();
        for row in self.rechunk_sources().await? {
            if seen.insert(row.doc_id.clone()) {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// The indexed documents with chunks that name no OCR engine
    /// ([`crate::processing::parser::OCR_ENGINE_KEY`]): every document that
    /// was not read by OCR, and scans indexed before the engine was recorded.
    pub async fn documents_without_ocr_engine(
        &self,
    ) -> Result<Vec<crate::storage::DocumentSourceRow>> {
        self.store
            .document_sources_where(Some(&no_ocr_engine_recorded()))
            .await
    }
    /// The metadata stored with the first chunk of `source` (empty when it
    /// has none).
    pub async fn document_metadata(&self, source: &str) -> Result<HashMap<String, String>> {
        let first = self
            .store
            .list_chunks(Some(&source_predicate(source)), 1)
            .await?;
        Ok(first
            .first()
            .and_then(|hit| serde_json::from_str(&hit.metadata_json).ok())
            .unwrap_or_default())
    }

    /// Brings the stored chunks of `source` to the current [`INDEX_FORMAT`]
    /// without reading its file: each chunk's context is made again from
    /// what was stored with it (title, section, unit kind; the opening from
    /// the document's first chunk), and the chunks are embedded and
    /// keyword-indexed again with it. Text, ids, pages, citations and refined
    /// tables stay as they are, and nothing is parsed: scans indexed before
    /// their OCR was cached are not read by OCR again. Possible for structure
    /// chunks only, whose context depends on nothing else; returns false,
    /// changing nothing, when a chunk is of another kind or there are none.
    /// Also false when a chunk is of an older [`CHUNKER_VERSION`]: its text
    /// is what the file is to be read and chunked again for, not kept.
    pub async fn upgrade_in_place(&mut self, source: &str) -> Result<bool> {
        self.upgrade_in_place_with(source, false).await
    }

    /// Records the chunks of `source` that are of an older [`CHUNKER_VERSION`]
    /// as kept on purpose (a scan that could be chunked again only by reading
    /// it with OCR again): their version becomes the current one, so they are
    /// no longer listed by [`Self::rechunk_sources`], and
    /// [`CHUNKS_KEPT_FROM_KEY`] records the version their text was made with
    /// (kept from an earlier keep when they have it). Only the metadata
    /// changes: text, vectors and ids stay. Returns the chunks marked.
    pub async fn keep_chunks_of_older_chunker(&mut self, source: &str) -> Result<u64> {
        // serde_json writes keys without spaces and escapes a quote inside a
        // value, so an entry appears verbatim and only as an entry. The
        // entries of version 1 go in front of the first one: the object's
        // opening `{` is replaced by the new entries (it is the first
        // character; one that is empty, `{}`, is never marked).
        let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
        let of_source = source_predicate(source);
        let current = format!("\"{CHUNKER_VERSION_KEY}\":\"{CHUNKER_VERSION}\"");
        let kept_marker = format!("%\"{CHUNKS_KEPT_FROM_KEY}\":\"%");
        let version_one = format!(
            "{{\"{CHUNKER_VERSION_KEY}\":\"{CHUNKER_VERSION}\",\"{CHUNKS_KEPT_FROM_KEY}\":\"1\","
        );
        // Version 1: no version entry at all.
        let mut marked = self
            .store
            .update_metadata_where(
                &format!(
                    "{of_source} AND metadata_json NOT LIKE {} AND metadata_json LIKE {}",
                    quote(&format!("%\"{CHUNKER_VERSION_KEY}\":\"%")),
                    quote("{\"%"),
                ),
                &format!("concat({}, substr(metadata_json, 2))", quote(&version_one)),
            )
            .await?;
        for older in (1..CHUNKER_VERSION).skip(1) {
            let entry = format!("\"{CHUNKER_VERSION_KEY}\":\"{older}\"");
            let has_entry = format!(
                "{of_source} AND metadata_json LIKE {}",
                quote(&format!("%{entry}%"))
            );
            // Kept before: the version their text is from is recorded already.
            marked += self
                .store
                .update_metadata_where(
                    &format!("{has_entry} AND metadata_json LIKE {}", quote(&kept_marker)),
                    &format!(
                        "replace(metadata_json, {}, {})",
                        quote(&entry),
                        quote(&current)
                    ),
                )
                .await?;
            marked += self
                .store
                .update_metadata_where(
                    &format!(
                        "{has_entry} AND metadata_json NOT LIKE {}",
                        quote(&kept_marker)
                    ),
                    &format!(
                        "replace(metadata_json, {}, {})",
                        quote(&entry),
                        quote(&format!("{current},\"{CHUNKS_KEPT_FROM_KEY}\":\"{older}\""))
                    ),
                )
                .await?;
        }
        tracing::info!(
            source,
            chunks = marked,
            "chunks of an older chunker kept: re-chunking would read the file with OCR again"
        );
        Ok(marked)
    }

    /// The source (`space_id`) the chunks of `source` belong to, from its
    /// first chunk; `None` when it has none.
    pub async fn source_owner(&self, source: &str) -> Result<Option<String>> {
        Ok(self
            .store
            .list_chunks(Some(&source_predicate(source)), 1)
            .await?
            .into_iter()
            .next()
            .map(|hit| hit.space_id))
    }

    /// [`Self::upgrade_in_place`] for a document whose chunks of an older
    /// [`CHUNKER_VERSION`] are kept on purpose: a scan that could be chunked
    /// again only by reading it with OCR again (its reading is not in the OCR
    /// cache). Their contexts are brought to the current [`INDEX_FORMAT`];
    /// their text and their chunker version stay as they are.
    pub async fn upgrade_format_keeping_chunks(&mut self, source: &str) -> Result<bool> {
        self.upgrade_in_place_with(source, true).await
    }

    async fn upgrade_in_place_with(
        &mut self,
        source: &str,
        older_chunks_kept: bool,
    ) -> Result<bool> {
        let current_chunker = CHUNKER_VERSION.to_string();
        let embedder = self
            .embeddings
            .clone()
            .ok_or_else(|| anyhow::Error::from(SearchModelsMissing))?;
        let mut hits = self
            .store
            .list_chunks(Some(&source_predicate(source)), 1_000_000)
            .await?;
        if hits.is_empty() {
            return Ok(false);
        }
        hits.sort_by(|a, b| {
            (a.doc_id.as_str(), a.chunk_index).cmp(&(b.doc_id.as_str(), b.chunk_index))
        });
        let count = |text: &str| {
            embedder
                .count_tokens(text)
                .unwrap_or_else(|| crate::embeddings::estimate_tokens(text))
        };
        let mut contexts = Vec::with_capacity(hits.len());
        let mut metadata = Vec::with_capacity(hits.len());
        for document in hits.chunk_by(|a, b| a.doc_id == b.doc_id) {
            let titled = DocumentContext::titled(document[0].title.as_str());
            let context = if document.len() > 1 {
                titled.with_opening(&document[0].text, &count)
            } else {
                titled
            };
            for hit in document {
                let meta: HashMap<String, String> = serde_json::from_str(&hit.metadata_json)
                    .with_context(|| format!("chunk {} has unreadable metadata", hit.id))?;
                if !older_chunks_kept && meta.get(CHUNKER_VERSION_KEY) != Some(&current_chunker) {
                    return Ok(false);
                }
                let section_path: Vec<String> = meta
                    .get("section_path")
                    .map(|path| path.split(" > ").map(str::to_string).collect())
                    .unwrap_or_default();
                let Some(chunk_context) = meta.get("unit_kind").and_then(|unit| {
                    crate::processing::structure_chunker::unit_context(
                        &context,
                        &section_path,
                        unit,
                    )
                }) else {
                    return Ok(false);
                };
                contexts.push(chunk_context);
                metadata.push(meta);
            }
        }

        let contextualized: Vec<String> = hits
            .iter()
            .zip(&contexts)
            .map(|(hit, context)| format!("{context}{}", hit.text))
            .collect();
        let texts: Vec<&str> = contextualized.iter().map(String::as_str).collect();
        let vectors = embedder.embed_documents(&texts)?;
        let now = chrono::Utc::now().timestamp();
        let mut records = Vec::with_capacity(hits.len());
        let mut fts_batch = Vec::with_capacity(hits.len());
        let mut chunk_ids = Vec::with_capacity(hits.len());
        for (((hit, context), mut meta), (vector, text)) in hits
            .into_iter()
            .zip(contexts)
            .zip(metadata)
            .zip(vectors.into_iter().zip(contextualized))
        {
            meta.insert(INDEX_FORMAT_KEY.to_string(), INDEX_FORMAT.to_string());
            meta.insert(CHUNK_CONTEXT_KEY.to_string(), context);
            chunk_ids.push(Uuid::parse_str(&hit.id).unwrap_or_else(|_| Uuid::new_v4()));
            fts_batch.push((hit.id.clone(), text, hit.title.clone(), hit.source.clone()));
            records.push(ChunkRecord {
                id: hit.id,
                doc_id: hit.doc_id,
                chunk_index: hit.chunk_index,
                text: hit.text,
                title: hit.title,
                source: hit.source,
                heading: hit.heading,
                vector,
                space_id: hit.space_id,
                metadata_json: serde_json::to_string(&meta)?,
                citation_json: hit.citation_json,
                created_at: now,
            });
        }
        let title = records[0].title.clone();
        let space_id = records[0].space_id.clone();
        self.remove_source_chunks(source).await?;
        self.store_prepared(PreparedDocument {
            title,
            space_id,
            records,
            fts_batch,
            chunk_ids,
        })
        .await?;
        tracing::info!(
            source,
            "chunks brought to the current index format in place"
        );
        Ok(true)
    }

    /// One row per indexed document with its source path, space and chunk count.
    pub async fn document_sources(&self) -> Result<Vec<crate::storage::DocumentSourceRow>> {
        self.store.document_sources().await
    }

    /// Stored source paths that `path` refers to.
    ///
    /// A full path is normalised like the indexer normalises it and must match
    /// a stored source exactly. A bare file name (no directory separator)
    /// matches every indexed file with that name, so callers can detect
    /// ambiguity. Returns an empty list when nothing is indexed under `path`.
    pub async fn find_indexed_sources(&self, path: &str) -> Result<Vec<String>> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let normalized = normalize_source_path(Path::new(trimmed));
        let predicate = format!("source = '{}'", normalized.replace('\'', "''"));
        let exact = self.store.list_chunks(Some(&predicate), 1).await?;
        if let Some(hit) = exact.into_iter().next() {
            return Ok(vec![hit.source]);
        }
        if normalized.contains('/') {
            return Ok(Vec::new());
        }
        let wanted = normalized.to_lowercase();
        let mut matches: Vec<String> = self
            .store
            .document_sources()
            .await?
            .into_iter()
            .map(|row| row.source)
            .filter(|source| {
                !source.contains("://")
                    && source
                        .rsplit('/')
                        .next()
                        .map(|name| name.to_lowercase() == wanted)
                        .unwrap_or(false)
            })
            .collect();
        matches.sort();
        matches.dedup();
        Ok(matches)
    }

    /// List all chunks matching an optional filter predicate (no vector search).
    /// This is the correct way to enumerate documents — NOT search_comprehensive("").
    /// Returns ComprehensiveResult for API compatibility.
    pub async fn list_documents(
        &self,
        filter: Option<MetadataFilter>,
        limit: usize,
    ) -> Result<Vec<ComprehensiveResult>> {
        let predicate = filter.as_ref().and_then(|f| f.to_lance_predicate());

        let hits = self.store.list_chunks(predicate.as_deref(), limit).await?;

        let mut results = Vec::with_capacity(hits.len());
        for hit in hits {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&hit.metadata_json).unwrap_or_default();
            let citation: Citation = serde_json::from_str(&hit.citation_json).unwrap_or_default();

            let mut full_metadata = metadata;
            full_metadata.insert("doc_id".to_string(), hit.doc_id.clone());
            full_metadata.insert("chunk_index".to_string(), hit.chunk_index.to_string());
            full_metadata.insert("source_file".to_string(), hit.source.clone());
            full_metadata.insert("space_id".to_string(), hit.space_id.clone());

            results.push(ComprehensiveResult {
                id: Uuid::parse_str(&hit.id).unwrap_or_default(),
                score: 0.0,
                metadata: full_metadata,
                citation,
                snippet: hit.text.clone(),
                source_index: "list".to_string(),
            });
        }

        Ok(results)
    }

    /// Raw LanceDB query — returns SearchHit objects without wrapping in ComprehensiveResult.
    /// Useful for ID lookups during deletion.
    pub async fn list_documents_raw(
        &self,
        predicate: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::storage::SearchHit>> {
        self.store.list_chunks(predicate, limit).await
    }

    /// The embedding model, when the search models are attached.
    pub fn embeddings(&self) -> Option<&dyn EmbeddingModel> {
        self.embeddings.as_deref()
    }

    /// A shared handle to the embedding model, for callers that embed outside the
    /// engine lock (inference is blocking; run it on a blocking thread).
    pub fn shared_embeddings(&self) -> Option<Arc<dyn EmbeddingModel>> {
        self.embeddings.clone()
    }

    /// Access to config
    pub fn config(&self) -> &RAGConfig {
        &self.config
    }

    /// Trigger index creation if needed (after large ingestion)
    pub async fn optimize(&self) -> Result<()> {
        // Compact LanceDB to remove tombstoned rows from previous deletions
        self.store.compact().await?;
        // Create vector index if enough rows exist
        self.store.create_index_if_needed().await
    }

    /// Rebuild the Tantivy full-text index from LanceDB.
    /// Used after schema migration wipes the old index, or to repair inconsistencies.
    pub async fn rebuild_text_index(&mut self) -> Result<()> {
        self.text_search.clear()?;

        let all_chunks = self.store.list_chunks(None, 1_000_000).await?;
        let total = all_chunks.len();

        if total == 0 {
            tracing::info!("No chunks in LanceDB — nothing to rebuild");
            return Ok(());
        }

        // Each chunk is keyword-indexed with the context it was indexed with,
        // as when it was stored.
        let batch: Vec<(String, String, String, String)> = all_chunks
            .into_iter()
            .map(|hit| {
                let metadata: HashMap<String, String> =
                    serde_json::from_str(&hit.metadata_json).unwrap_or_default();
                let text = match metadata.get(CHUNK_CONTEXT_KEY) {
                    Some(context) => format!("{context}{}", hit.text),
                    None => hit.text,
                };
                (hit.id, text, hit.title, hit.source)
            })
            .collect();

        self.text_search.index_chunks_batch(&batch)?;
        self.text_search.commit()?;

        let indexed = self.text_search.count().unwrap_or(0);
        tracing::info!(
            total_chunks = total,
            indexed = indexed,
            "Tantivy index rebuilt from LanceDB"
        );
        Ok(())
    }

    /// Remove near-duplicate results using Jaccard similarity on word sets.
    /// Only deduplicates chunks from the SAME source file (overlapping windows).
    /// Chunks from different files are never merged — even if they have similar
    /// boilerplate (e.g. invoices with the same company header).
    fn deduplicate_results(results: &mut Vec<ComprehensiveResult>, threshold: f32) {
        use std::collections::HashSet;
        let word_sets: Vec<HashSet<&str>> = results
            .iter()
            .map(|r| r.snippet.split_whitespace().collect::<HashSet<_>>())
            .collect();

        let sources: Vec<String> = results
            .iter()
            .map(|r| r.metadata.get("source_file").cloned().unwrap_or_default())
            .collect();

        let mut keep = Vec::new();
        for i in 0..results.len() {
            let mut is_dup = false;
            for &j in &keep {
                // Only dedup within the same source file
                if sources[i] != sources[j] {
                    continue;
                }
                let intersection = word_sets[i].intersection(&word_sets[j]).count();
                let union = word_sets[i].union(&word_sets[j]).count();
                if union > 0 && (intersection as f32 / union as f32) > threshold {
                    is_dup = true;
                    break;
                }
            }
            if !is_dup {
                keep.push(i);
            }
        }

        // Remove duplicates in reverse index order to preserve positions.
        let keep_set: HashSet<usize> = keep.into_iter().collect();
        let mut i = results.len();
        while i > 0 {
            i -= 1;
            if !keep_set.contains(&i) {
                results.swap_remove(i);
            }
        }
        // Restore original order by score (descending)
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// On a query about citations (the only queries that see bibliography
    /// entries), entries are kept below content chunks unless one outscores
    /// every content chunk (a query about a cited work). Order is otherwise
    /// unchanged.
    fn demote_reference_entries(results: &mut Vec<ComprehensiveResult>) {
        let is_reference = |r: &ComprehensiveResult| is_reference_entry(&r.metadata);
        let best_content = results
            .iter()
            .filter(|r| !is_reference(r))
            .map(|r| r.score)
            .fold(f32::NEG_INFINITY, f32::max);
        if best_content == f32::NEG_INFINITY {
            return;
        }
        let (mut head, tail): (Vec<_>, Vec<_>) = std::mem::take(results)
            .into_iter()
            .partition(|r| !is_reference(r) || r.score > best_content);
        head.extend(tail);
        *results = head;
    }

    /// Replaces each result's score with the cross-encoder's relevance and sorts
    /// by it. Candidates the cross-encoder could not tokenize keep their fused
    /// score.
    ///
    /// The cross-encoder returns logits: unbounded, and negative for most
    /// passages that do not answer the query. They are mapped through the
    /// sigmoid to a relevance in 0..1 (the order is unchanged), the same range
    /// as the fused scores, so that [`Self::apply_mmr_diversity`]'s
    /// multiplicative decay lowers a repeated source's score. Applied to a
    /// negative logit the decay raised it instead, by more for each further
    /// chunk of the same file, and a file's weakest chunks overtook the best
    /// passages of every other file.
    fn apply_rerank_scores(results: &mut [ComprehensiveResult], logits: &HashMap<String, f32>) {
        for result in results.iter_mut() {
            if let Some(&logit) = logits.get(&result.id.to_string()) {
                result.score = sigmoid(logit);
            }
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Diminishing returns per source file: each further chunk of a file, in
    /// score order, gets `score *= lambda^count` (count = chunks of that file
    /// already seen), then the results are re-sorted. This balances depth
    /// (several chunks of one file) against spreading across files without a
    /// hard cap. Scores must be non-negative, as fused scores and the
    /// cross-encoder relevance from [`Self::apply_rerank_scores`] are; on a
    /// negative score the decay would raise it.
    fn apply_mmr_diversity(results: &mut [ComprehensiveResult], lambda: f32) {
        let mut source_seen: HashMap<String, u32> = HashMap::new();
        for result in results.iter_mut() {
            let source = result
                .metadata
                .get("source_file")
                .cloned()
                .unwrap_or_default();
            let count = source_seen.entry(source).or_insert(0);
            if *count > 0 {
                result.score *= lambda.powi(*count as i32);
            }
            *count += 1;
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
}

#[cfg(test)]
impl RAGEngine {
    /// Stores `file` as a build of [`INDEX_FORMAT`] 1 did: chunks without a
    /// format or a stored context, keyword-indexed with their text only. The
    /// chunker version is kept: for tests of an upgrade from an index made by
    /// an earlier version whose chunk text needs no change, only the context.
    pub(crate) async fn commit_file_as_format_one(
        &mut self,
        mut file: PreparedFile,
    ) -> Result<Vec<Uuid>> {
        let mut texts: HashMap<String, String> = HashMap::new();
        for record in &mut file.document.records {
            let mut metadata: HashMap<String, String> =
                serde_json::from_str(&record.metadata_json)?;
            metadata.remove(INDEX_FORMAT_KEY);
            metadata.remove(CHUNK_CONTEXT_KEY);
            record.metadata_json = serde_json::to_string(&metadata)?;
            texts.insert(record.id.clone(), record.text.clone());
        }
        for (id, text, _, _) in &mut file.document.fts_batch {
            if let Some(plain) = texts.get(id) {
                text.clone_from(plain);
            }
        }
        self.no_stale_chunks
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.commit_file(file).await
    }

    /// Stores `file` as a build before [`CHUNKER_VERSION`] 2 did: chunks
    /// without a chunker version, in the current [`INDEX_FORMAT`], each with
    /// the text `old_text` makes of the text chunked now (an earlier chunker
    /// that lost part of it). For tests of the re-chunking of such an index.
    pub(crate) async fn commit_file_as_chunker_one(
        &mut self,
        mut file: PreparedFile,
        old_text: impl Fn(&str) -> String,
    ) -> Result<Vec<Uuid>> {
        for record in &mut file.document.records {
            let mut metadata: HashMap<String, String> =
                serde_json::from_str(&record.metadata_json)?;
            metadata.remove(CHUNKER_VERSION_KEY);
            record.metadata_json = serde_json::to_string(&metadata)?;
            record.text = old_text(&record.text);
        }
        self.no_rechunk_chunks
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.commit_file(file).await
    }
}

#[cfg(test)]
impl RAGEngine {
    /// Whether [`Self::rechunk_sources`] found nothing to re-chunk and skips
    /// its scan over every chunk's metadata from now on.
    pub(crate) fn rechunk_latched(&self) -> bool {
        self.no_rechunk_chunks
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
impl PreparedFile {
    /// The text of each chunk prepared, in order.
    pub(crate) fn chunk_texts(&self) -> Vec<String> {
        self.document
            .records
            .iter()
            .map(|record| record.text.clone())
            .collect()
    }
}

#[cfg(test)]
mod page_metadata_tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn mixed_separators_resolve_to_one_identity() {
        let a = normalize_source_path(Path::new(
            "C:/Users/V/Research Papers\\Test-Time Learning\\RoboTTT.pdf",
        ));
        let b = normalize_source_path(Path::new(
            "C:\\Users\\V\\Research Papers\\Test-Time Learning\\RoboTTT.pdf",
        ));
        let c = normalize_source_path(Path::new(
            "\\\\?\\C:\\Users\\V\\Research Papers\\.\\x\\..\\Test-Time Learning\\RoboTTT.pdf",
        ));
        assert_eq!(
            a,
            "c:/users/v/research papers/test-time learning/robottt.pdf"
        );
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(
            canonical_path(Path::new("C:/Users/V/Research Papers\\a.pdf")),
            std::path::PathBuf::from("C:\\Users\\V\\Research Papers\\a.pdf")
        );
        assert_eq!(
            canonical_path(Path::new("\\\\?\\UNC\\server\\share\\a.pdf")),
            std::path::PathBuf::from("\\\\server\\share\\a.pdf")
        );
    }

    #[cfg(windows)]
    #[test]
    fn legacy_spellings_cover_verbatim_and_forward_slash_forms() {
        let path = Path::new("C:\\Papers\\A.pdf");
        let canonical = normalize_source_path(path);
        let legacy = legacy_source_spellings(path, &canonical);
        assert!(legacy.contains(&"C:\\Papers\\A.pdf".to_string()));
        assert!(legacy.contains(&"C:/Papers/A.pdf".to_string()));
        assert!(!legacy.contains(&canonical));

        // The previous normalization kept verbatim prefixes and `.` parts.
        let verbatim = Path::new(r"\\?\C:\Papers\.\A.pdf");
        let canonical = normalize_source_path(verbatim);
        assert_eq!(canonical, "c:/papers/a.pdf");
        assert!(legacy_source_spellings(verbatim, &canonical)
            .contains(&"//?/c:/papers/./a.pdf".to_string()));
    }

    #[test]
    fn layout_metadata_round_trips_to_citation_pages() {
        use crate::processing::document_model::BBox;
        use crate::processing::structure_chunker::{ChunkLayout, ChunkRegion};
        let layout = ChunkLayout {
            page_start: Some(3),
            page_end: Some(4),
            regions: vec![
                ChunkRegion {
                    page: 3,
                    bbox: BBox::new(72.04, 400.0, 300.0, 700.0),
                },
                ChunkRegion {
                    page: 4,
                    bbox: BBox::new(72.0, 600.0, 300.0, 720.0),
                },
            ],
            section_path: vec!["3 Method".to_string(), "3.2 Chunkwise form".to_string()],
            block_kinds: vec!["paragraph", "equation"],
            unit: "text",
            incomplete: false,
        };
        let mut meta = HashMap::new();
        insert_layout_metadata(&mut meta, &layout);
        assert_eq!(page_numbers_from_metadata(&meta).as_deref(), Some("3-4"));
        assert_eq!(meta["section_path"], "3 Method > 3.2 Chunkwise form");
        assert_eq!(meta["block_kinds"], "paragraph,equation");
        assert_eq!(meta["unit_kind"], "text");
        assert!(!meta.contains_key(TABLE_INCOMPLETE_KEY));
        let regions: Vec<ChunkRegion> = serde_json::from_str(&meta["bboxes"]).expect("bbox json");
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].page, 3);
        assert_eq!(regions[0].bbox.x0, 72.0);
        assert!(meta["bboxes"].contains("\"page\":3,\"x0\":72.0"));
    }

    fn result(score: f32, unit: &str) -> ComprehensiveResult {
        let mut metadata = HashMap::new();
        metadata.insert("unit_kind".to_string(), unit.to_string());
        ComprehensiveResult {
            id: Uuid::new_v4(),
            score,
            metadata,
            citation: Citation::default(),
            snippet: unit.to_string(),
            source_index: "hybrid".to_string(),
        }
    }

    fn chunk_of(source: &str, chunk_index: u32, fused: f32) -> ComprehensiveResult {
        let mut metadata = HashMap::new();
        metadata.insert("source_file".to_string(), source.to_string());
        metadata.insert("chunk_index".to_string(), chunk_index.to_string());
        ComprehensiveResult {
            id: Uuid::new_v4(),
            score: fused,
            metadata,
            citation: Citation::default(),
            snippet: format!("{source} chunk {chunk_index}"),
            source_index: "hybrid".to_string(),
        }
    }

    #[test]
    fn diversity_after_reranking_never_lifts_a_files_weak_chunks() {
        // Cross-encoder logits observed for "How large is the security deposit
        // under the Saffron Estates lease?": the lease's title chunk, then its
        // rent-and-deposit chunk (the answer), then chunks that do not answer
        // the question, most of them from one invoice.
        let scored = [
            ("lease.pdf", 0, 2.4448),
            ("lease.pdf", 1, -3.8898),
            ("invoice.pdf", 5, -10.4067),
            ("lease.pdf", 2, -10.6107),
            ("invoice.pdf", 3, -10.6670),
            ("invoice.pdf", 6, -10.7457),
            ("invoice.pdf", 4, -10.7980),
            ("study.md", 1, -11.1144),
            ("invoice.pdf", 2, -11.3587),
            ("invoice.pdf", 0, -11.3590),
            ("invoice.pdf", 1, -11.4041),
        ];
        // Fused order differs from the cross-encoder's, as in the real search.
        let mut results: Vec<ComprehensiveResult> = scored
            .iter()
            .enumerate()
            .map(|(i, (source, chunk, _))| chunk_of(source, *chunk, 1.0 - 0.05 * i as f32))
            .rev()
            .collect();
        let logits: HashMap<String, f32> = results
            .iter()
            .zip(scored.iter().rev())
            .map(|(r, (_, _, logit))| (r.id.to_string(), *logit))
            .collect();

        RAGEngine::apply_rerank_scores(&mut results, &logits);
        let relevance: HashMap<Uuid, f32> = results.iter().map(|r| (r.id, r.score)).collect();

        RAGEngine::apply_mmr_diversity(&mut results, 0.5);
        let order: Vec<(&str, &str)> = results
            .iter()
            .map(|r| {
                (
                    r.metadata["source_file"].as_str(),
                    r.metadata["chunk_index"].as_str(),
                )
            })
            .collect();

        // The answer stays right below the title chunk of the same lease; the
        // invoice's repeated chunks do not climb over it.
        assert_eq!(order[..2], [("lease.pdf", "0"), ("lease.pdf", "1")]);
        assert_eq!(order[2], ("invoice.pdf", "5"));
        // Diversity only ever lowers a score.
        for r in &results {
            assert!(r.score <= relevance[&r.id], "{:?} rose", r.metadata);
        }
        // Within a file, the cross-encoder's order is kept.
        let invoice: Vec<&str> = order
            .iter()
            .filter(|(s, _)| *s == "invoice.pdf")
            .map(|(_, c)| *c)
            .collect();
        assert_eq!(invoice, ["5", "3", "6", "4", "2", "0", "1"]);
        // Relevance has the fused scores' range.
        assert!(relevance.values().all(|s| (0.0..=1.0).contains(s)));
    }

    #[test]
    fn reference_entries_rank_below_content_unless_they_beat_it() {
        // Input is score-sorted, as after reranking and MMR.
        let mut results = vec![
            result(0.95, "reference_entry"),
            result(0.9, "text"),
            result(0.5, "reference_entry"),
            result(0.4, "text"),
        ];
        RAGEngine::demote_reference_entries(&mut results);
        let order: Vec<(f32, String)> = results
            .iter()
            .map(|r| (r.score, r.metadata["unit_kind"].clone()))
            .collect();
        let expected: Vec<(f32, String)> = [
            (0.95, "reference_entry"),
            (0.9, "text"),
            (0.4, "text"),
            (0.5, "reference_entry"),
        ]
        .iter()
        .map(|(s, k)| (*s, k.to_string()))
        .collect();
        assert_eq!(order, expected);
    }

    #[test]
    fn only_queries_about_citations_seek_references() {
        for query in [
            "which paper proposed the delta rule",
            "Who introduced linear attention?",
            "what does the DeltaNet paper cite",
            "papers citing Katharopoulos et al.",
            "list the references of the RWKV paper",
            "bibliography of the survey",
            "the original paper on fast weights",
        ] {
            assert!(seeks_references(query), "{query}");
        }
        for query in [
            "delta rule parallelized over sequence length",
            "how is the chunkwise form computed",
            "what is the recall of HNSW",
            "who won the benchmark",
        ] {
            assert!(!seeks_references(query), "{query}");
        }
    }

    /// A scan indexed from the OCR cache loses its entry when it leaves the
    /// index, by file or by its source's space.
    #[tokio::test]
    async fn ocr_cache_entries_leave_the_cache_with_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        // PP-OCRv6 only, and it is not installed: the scans index only from the cache.
        config.ocr.engine = crate::config::OcrEngineChoice::PpOcr;
        let mut engine = RAGEngine::new(config).await.unwrap();
        engine
            .attach_search_models(SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .unwrap();
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../shodh-eval/data/scanned/corpus/contracts/lease-saffron-tidewater-scan.pdf");
        let index = |name: &str| {
            let path = dir.path().join(name);
            std::fs::copy(&corpus, &path).unwrap();
            path
        };
        let cache = engine.ocr_cache.clone();

        let by_file = index("a-scan.pdf");
        let source = normalize_source_path(&by_file);
        cache
            .put(&crate::processing::parser::tests::cached_scan(
                &by_file,
                "Rent is due monthly",
            ))
            .unwrap();
        engine
            .add_document_from_file(
                &by_file,
                HashMap::from([("space_id".to_string(), "s1".to_string())]),
            )
            .await
            .expect("indexed from the OCR cache");
        assert!(cache.get(&source).is_some());
        engine
            .delete_by_source(&by_file.to_string_lossy())
            .await
            .unwrap();
        assert!(cache.get(&source).is_none(), "removed with its file");

        let by_space = index("b-scan.pdf");
        let source = normalize_source_path(&by_space);
        cache
            .put(&crate::processing::parser::tests::cached_scan(
                &by_space,
                "Rent is due monthly",
            ))
            .unwrap();
        engine
            .add_document_from_file(
                &by_space,
                HashMap::from([("space_id".to_string(), "s2".to_string())]),
            )
            .await
            .unwrap();
        engine.delete_by_space_id("s1").await.unwrap();
        assert!(cache.get(&source).is_some(), "another space's file stays");
        engine.delete_by_space_id("s2").await.unwrap();
        assert!(cache.get(&source).is_none(), "removed with its space");
    }

    async fn engine_with_paper(dir: &Path) -> RAGEngine {
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.join("data");
        config.embedding.model_dir = dir.join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        let mut engine = RAGEngine::new(config).await.unwrap();
        engine
            .attach_search_models(SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .unwrap();
        // The reported case: the References section shares the query's words with
        // the introduction and outscored it.
        let paper = dir.join("deltanet.tex");
        std::fs::write(
            &paper,
            r"\documentclass{article}
\begin{document}
\section{Introduction}
We show how the delta rule update of linear transformers is parallelized over
sequence length with a chunkwise form, so training on long sequences is efficient.
\section{Experiments}
Language models trained on long documents reach lower perplexity.
\begin{thebibliography}{9}
\bibitem{yang} S. Yang. Parallelizing the delta rule over sequence length. 2024.
\bibitem{schlag} I. Schlag. The delta rule over sequence length in fast weight programmers. 2021.
\end{thebibliography}
\end{document}
",
        )
        .unwrap();
        engine
            .add_document_from_file(&paper, HashMap::new())
            .await
            .unwrap();
        engine
    }

    fn test_engine_config(dir: &Path) -> RAGConfig {
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.join("data");
        config.embedding.model_dir = dir.join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        config
    }

    async fn test_engine(dir: &Path) -> RAGEngine {
        let mut engine = RAGEngine::new(test_engine_config(dir)).await.unwrap();
        engine
            .attach_search_models(SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .unwrap();
        engine
    }

    /// Evidence tables with a row, a filled cell and a cache entry for each of `papers`.
    struct SeededTables {
        store: Arc<crate::research::tables::TableStore>,
        table_id: String,
        column_key: String,
        papers: Vec<String>,
    }

    fn seeded_tables(dir: &Path, papers: &[&Path]) -> SeededTables {
        use crate::research::tables::{
            Cell, CellStatus, CellValue, ColumnKind, ColumnSpec, TableStore,
        };
        use crate::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};
        let database = dir.join("shodh.db");
        let workspace = WorkspaceStore::open(&database, None)
            .unwrap()
            .create(
                &NewWorkspace {
                    name: "Survey".to_string(),
                    ..Default::default()
                },
                WorkspaceAuthor::User,
            )
            .unwrap()
            .id;
        let store = Arc::new(TableStore::open(&database, None).unwrap());
        let table = store.create_table(&workspace, "Datasets").unwrap();
        let column = store
            .add_column(
                &table.id,
                &ColumnSpec {
                    question: "Which dataset?".to_string(),
                    kind: ColumnKind::Text,
                    options: Vec::new(),
                    instructions: String::new(),
                },
            )
            .unwrap();
        let papers: Vec<String> = papers
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let rows = store.add_rows(&table.id, &papers).unwrap();
        for (i, row) in rows.iter().enumerate() {
            let hash = format!("hash-{i}");
            let cell = Cell {
                table_id: table.id.clone(),
                row_id: row.id.clone(),
                column_id: column.id.clone(),
                status: CellStatus::Verified,
                value: Some(CellValue::Text {
                    text: "LAION-400M".to_string(),
                }),
                quote: Some("We pre-train on LAION-400M".to_string()),
                chunk_id: None,
                page: Some(3),
                bboxes: None,
                model: Some("test-model".to_string()),
                column_revision: Some(column.revision),
                paper_hash: Some(hash.clone()),
                filled_at: Some("2026-10-22T00:00:00.000Z".to_string()),
                edited_at: None,
                error: None,
            };
            let entry = crate::research::tables::CacheEntry {
                paper_hash: hash,
                column_key: column.key.clone(),
                model: "test-model".to_string(),
                response_json: "{\"answer\":\"LAION-400M\"}".to_string(),
            };
            assert!(store.store_fill(&cell, Some(&entry)).unwrap());
        }
        SeededTables {
            store,
            table_id: table.id,
            column_key: column.key,
            papers,
        }
    }

    impl SeededTables {
        /// Whether paper `i` still has its row, its cell and its cached quote (all
        /// three, or none: a paper is never half forgotten).
        fn keeps(&self, i: usize) -> bool {
            let key = crate::research::path_key(&self.papers[i]);
            let row = self
                .store
                .rows(&self.table_id)
                .unwrap()
                .into_iter()
                .find(|r| r.document_id == key);
            let cells = self.store.cells(&self.table_id).unwrap();
            let cell = row
                .as_ref()
                .is_some_and(|row| cells.iter().any(|c| c.row_id == row.id));
            let cached = self
                .store
                .cached(&format!("hash-{i}"), &self.column_key, "test-model")
                .unwrap()
                .is_some();
            assert_eq!(row.is_some(), cell, "row and cell of paper {i}");
            assert_eq!(row.is_some(), cached, "row and cached quote of paper {i}");
            row.is_some()
        }
    }

    /// Writes and indexes a note about `word` as `dir/name` in `space`.
    async fn index_note(
        engine: &mut RAGEngine,
        dir: &Path,
        name: &str,
        word: &str,
        space: &str,
    ) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            &path,
            format!(
                "# Notes\n\nThe {word} was seen near the river at dawn. The {word} returned \
                 the next morning, and the field notes describe the {word} at length.\n"
            ),
        )
        .unwrap();
        let metadata = if space.is_empty() {
            HashMap::new()
        } else {
            HashMap::from([("space_id".to_string(), space.to_string())])
        };
        engine
            .add_document_from_file(&path, metadata)
            .await
            .unwrap();
        path
    }

    /// An engine reporting its removals to evidence tables that hold papers `a`, `b`
    /// and `c` (indexed in the spaces `spaces`).
    async fn engine_with_tables(
        dir: &Path,
        spaces: [&str; 3],
    ) -> (RAGEngine, SeededTables, [std::path::PathBuf; 3]) {
        let mut engine = test_engine(dir).await;
        let papers = dir.join("papers");
        let a = index_note(&mut engine, &papers, "a.md", "zebra", spaces[0]).await;
        let b = index_note(&mut engine, &papers, "b.md", "otter", spaces[1]).await;
        let c = index_note(&mut engine, &papers, "c.md", "heron", spaces[2]).await;
        let tables = seeded_tables(dir, &[&a, &b, &c]);
        engine.set_removal_observer(Some(tables.store.clone()));
        (engine, tables, [a, b, c])
    }

    /// Forget erases everywhere: every way a paper leaves the index also removes its
    /// rows, cells and cached quotes, and leaves the other papers' alone.
    #[tokio::test]
    async fn removing_a_file_by_its_path_forgets_it_in_the_tables() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, [a, ..]) =
            engine_with_tables(dir.path(), ["s1", "s1", "s1"]).await;
        // Another spelling of the path reaches the same row.
        engine
            .delete_by_source(&a.to_string_lossy().replace('\\', "/"))
            .await
            .unwrap();
        assert!(!tables.keeps(0));
        assert!(tables.keeps(1) && tables.keeps(2));
    }

    #[tokio::test]
    async fn removing_a_document_by_its_id_forgets_it_in_the_tables() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, [_, b, _]) =
            engine_with_tables(dir.path(), ["s1", "s1", "s1"]).await;
        let source = normalize_source_path(&b);
        let doc_id = engine
            .document_sources()
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.source == source)
            .unwrap()
            .doc_id;
        engine.delete_by_doc_id(&doc_id).await.unwrap();
        assert!(!tables.keeps(1));
        assert!(tables.keeps(0) && tables.keeps(2));
    }

    #[tokio::test]
    async fn removing_a_space_forgets_its_files_in_the_tables() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, _) = engine_with_tables(dir.path(), ["s1", "s2", "s1"]).await;
        engine.delete_by_space_id("s1").await.unwrap();
        assert!(!tables.keeps(0) && !tables.keeps(2));
        assert!(tables.keeps(1));
    }

    #[tokio::test]
    async fn removing_a_library_source_forgets_its_files_and_the_unowned_ones_under_it() {
        let dir = tempfile::tempdir().unwrap();
        // `b` belongs to no source (indexed before chunks recorded theirs) and lies
        // under the removed folder; `c` is the source of another folder.
        let (mut engine, tables, _) = engine_with_tables(dir.path(), ["papers", "", "other"]).await;
        let folder = dir.path().join("papers").display().to_string();
        engine
            .delete_library_source("papers", Some(folder.as_str()), &[])
            .await
            .unwrap();
        assert!(!tables.keeps(0) && !tables.keeps(1));
        assert!(tables.keeps(2));
    }

    #[tokio::test]
    async fn clearing_the_index_forgets_every_file_in_the_tables() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, _) = engine_with_tables(dir.path(), ["s1", "s2", ""]).await;
        engine.clear_all_data().await.unwrap();
        assert!(!tables.keeps(0) && !tables.keeps(1) && !tables.keeps(2));
    }

    #[tokio::test]
    async fn indexing_a_file_again_does_not_forget_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, [a, ..]) =
            engine_with_tables(dir.path(), ["s1", "s1", "s1"]).await;
        engine
            .add_document_from_file(&a, HashMap::new())
            .await
            .unwrap();
        assert!(tables.keeps(0) && tables.keeps(1) && tables.keeps(2));
    }

    /// An observer that fails while `failing` is set.
    struct FlakyObserver {
        inner: Arc<crate::research::tables::TableStore>,
        failing: std::sync::atomic::AtomicBool,
    }

    impl LibraryRemovalObserver for FlakyObserver {
        fn forget_sources(&self, sources: &[String]) -> Result<()> {
            if self.failing.load(std::sync::atomic::Ordering::SeqCst) {
                anyhow::bail!("the database is locked (injected)");
            }
            self.inner.forget_sources(sources)
        }

        fn forget_all(&self) -> Result<()> {
            if self.failing.load(std::sync::atomic::Ordering::SeqCst) {
                anyhow::bail!("the database is locked (injected)");
            }
            LibraryRemovalObserver::forget_all(&*self.inner)
        }
    }

    /// An observer that counts what it is told.
    #[derive(Default)]
    struct CountingObserver {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl LibraryRemovalObserver for CountingObserver {
        fn forget_sources(&self, _: &[String]) -> Result<()> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn forget_all(&self) -> Result<()> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    fn pending_file(dir: &Path) -> std::path::PathBuf {
        dir.join("data").join(PENDING_FORGETS_FILE)
    }

    #[tokio::test]
    async fn a_reset_works_on_an_index_that_cannot_be_listed_and_forgets_everything() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, _) = engine_with_tables(dir.path(), ["s1", "s2", ""]).await;
        engine
            .fail_source_listing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        engine
            .clear_all_data()
            .await
            .expect("the reset still clears");
        assert!(engine.document_sources().await.unwrap().is_empty());
        assert!(!tables.keeps(0) && !tables.keeps(1) && !tables.keeps(2));
        assert!(
            tables.store.table(&tables.table_id).unwrap().is_some()
                && tables.store.columns(&tables.table_id).unwrap().len() == 1,
            "tables and columns stay"
        );
        assert!(!pending_file(dir.path()).exists());
    }

    #[tokio::test]
    async fn a_reset_the_tables_did_not_take_is_recorded_and_settled() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, _) = engine_with_tables(dir.path(), ["s1", "s2", ""]).await;
        let flaky = Arc::new(FlakyObserver {
            inner: tables.store.clone(),
            failing: std::sync::atomic::AtomicBool::new(true),
        });
        engine.set_removal_observer(Some(flaky.clone()));
        engine
            .fail_source_listing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        engine.clear_all_data().await.unwrap();
        assert!(tables.keeps(0) && tables.keeps(1) && tables.keeps(2));
        assert!(pending_file(dir.path()).exists());
        flaky
            .failing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        engine.settle_forgotten().await;
        assert!(!tables.keeps(0) && !tables.keeps(1) && !tables.keeps(2));
        assert!(!pending_file(dir.path()).exists());
    }

    #[tokio::test]
    async fn a_removal_that_fails_after_the_index_lost_the_file_still_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, [a, ..]) =
            engine_with_tables(dir.path(), ["s1", "s1", "s1"]).await;
        engine
            .fail_text_removal
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(engine.delete_by_source(&a.to_string_lossy()).await.is_err());
        assert!(
            !tables.keeps(0),
            "gone from the index, so gone from the tables"
        );
        assert!(tables.keeps(1) && tables.keeps(2));
    }

    #[tokio::test]
    async fn a_corrupt_pending_record_is_kept_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, [a, ..]) =
            engine_with_tables(dir.path(), ["s1", "s1", "s1"]).await;
        std::fs::write(pending_file(dir.path()), b"[\"half").unwrap();
        engine.delete_by_source(&a.to_string_lossy()).await.unwrap();
        assert!(!tables.keeps(0));
        let aside: Vec<_> = std::fs::read_dir(dir.path().join("data"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("pending_forgets.corrupt-")
            })
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read(aside[0].path()).unwrap(), b"[\"half");
        assert!(!pending_file(dir.path()).exists());
    }

    #[tokio::test]
    async fn a_calendar_item_is_not_reported_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let observer = Arc::new(CountingObserver::default());
        engine.set_removal_observer(Some(observer.clone()));
        engine.delete_by_source("calendar://task/7").await.unwrap();
        engine.delete_by_source("note://n1").await.unwrap();
        assert_eq!(observer.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!pending_file(dir.path()).exists());
    }

    #[tokio::test]
    async fn a_file_with_chunks_in_two_spaces_survives_the_removal_of_one() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, [a, ..]) =
            engine_with_tables(dir.path(), ["s1", "s1", "s1"]).await;
        let source = normalize_source_path(&a);
        let copies: Vec<crate::types::ChunkRecord> = engine
            .store
            .list_chunks(Some(&source_predicate(&source)), 100)
            .await
            .unwrap()
            .into_iter()
            .map(|chunk| crate::types::ChunkRecord {
                id: format!("{}-copy", chunk.id),
                doc_id: "copy-doc".to_string(),
                chunk_index: chunk.chunk_index,
                text: chunk.text,
                title: chunk.title,
                source: chunk.source,
                heading: chunk.heading,
                vector: vec![0.0; engine.config.embedding.dimension],
                space_id: "s2".to_string(),
                metadata_json: chunk.metadata_json,
                citation_json: chunk.citation_json,
                created_at: 0,
            })
            .collect();
        assert!(!copies.is_empty());
        engine.store.upsert_chunks(copies).await.unwrap();
        engine.delete_by_space_id("s1").await.unwrap();
        assert!(tables.keeps(0), "still in the library through space s2");
        assert!(!tables.keeps(1) && !tables.keeps(2));
    }

    #[tokio::test]
    async fn a_removal_the_tables_did_not_take_is_written_down_and_retried() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, [a, ..]) =
            engine_with_tables(dir.path(), ["s1", "s1", "s1"]).await;
        let flaky = Arc::new(FlakyObserver {
            inner: tables.store.clone(),
            failing: std::sync::atomic::AtomicBool::new(true),
        });
        engine.set_removal_observer(Some(flaky.clone()));
        engine
            .delete_by_source(&a.to_string_lossy())
            .await
            .expect("the index removal itself succeeded");
        assert!(tables.keeps(0), "not forgotten while the observer fails");
        assert_eq!(engine.settle_forgotten().await, 0);
        assert!(tables.keeps(0));

        assert!(
            dir.path().join("data").join(PENDING_FORGETS_FILE).exists(),
            "written down, for a run after a quit"
        );
        // The observer recovers; the next start (or removal) reports the file again.
        flaky
            .failing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // A new run: a new engine over the same data directory reads the record.
        drop(engine);
        let mut engine = test_engine(dir.path()).await;
        engine.set_removal_observer(Some(flaky));
        assert_eq!(engine.settle_forgotten().await, 1);
        assert!(!tables.keeps(0));
        assert!(tables.keeps(1) && tables.keeps(2));
        assert!(
            !dir.path().join("data").join(PENDING_FORGETS_FILE).exists(),
            "the record goes once the tables confirmed"
        );
    }

    #[tokio::test]
    async fn a_file_added_back_before_the_retry_is_not_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, tables, [a, ..]) =
            engine_with_tables(dir.path(), ["s1", "s1", "s1"]).await;
        let flaky = Arc::new(FlakyObserver {
            inner: tables.store.clone(),
            failing: std::sync::atomic::AtomicBool::new(true),
        });
        engine.set_removal_observer(Some(flaky.clone()));
        engine.delete_by_source(&a.to_string_lossy()).await.unwrap();
        engine
            .add_document_from_file(&a, HashMap::new())
            .await
            .unwrap();
        flaky
            .failing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(engine.settle_forgotten().await, 0);
        assert!(tables.keeps(0), "it is in the library again");
        assert!(!dir.path().join("data").join(PENDING_FORGETS_FILE).exists());
    }

    /// A file with text is never indexed as no chunks: a short note is one
    /// chunk, a CJK note is chunked by unit, and a note the structure chunker
    /// finds no unit in (a heading alone) falls back to windows.
    #[tokio::test]
    async fn short_and_whitespace_free_files_are_indexed_with_their_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let cases = [
            ("reply.txt", "Yes.\n", "Yes.", false),
            (
                "kaimono.md",
                "# 買い物\n\n牛乳と卵を買う。\n",
                "牛乳と卵を買う。",
                true,
            ),
            ("list.md", "- Milk\n", "Milk", true),
            ("heading.md", "# Shopping list\n", "Shopping list", false),
        ];
        for (name, content, _, _) in &cases {
            let path = dir.path().join(name);
            std::fs::write(&path, content).unwrap();
            let ids = engine
                .add_document_from_file(&path, HashMap::new())
                .await
                .unwrap();
            assert_eq!(ids.len(), 1, "{name}");
        }
        let chunks = engine.store.list_chunks(None, 100).await.unwrap();
        for (name, _, text, by_unit) in cases {
            let chunk = chunks
                .iter()
                .find(|c| c.source.ends_with(name))
                .unwrap_or_else(|| panic!("no chunk of {name}"));
            assert!(chunk.text.contains(text), "{name}: {}", chunk.text);
            let metadata: HashMap<String, String> =
                serde_json::from_str(&chunk.metadata_json).unwrap();
            assert_eq!(
                metadata.get("chunker").map(String::as_str),
                by_unit.then_some(STRUCTURE_CHUNKER_VERSION),
                "{name}"
            );
        }
    }

    const LEASE: &str =
        "Lessor: Saffron Estates Pvt. Ltd. Lessee: Tidewater Logistics Pvt. Ltd.\n\n\
                         # Rent\n\n\
                         Monthly rent is INR 4,85,000, payable by the 5th day of each month.\n";

    /// Removing a Library source deletes what it owns and, under its folder,
    /// only what no source owns: never a neighbour whose path starts the same
    /// ("Papers 2024" for "Papers"), a source listed inside it, or a file
    /// added in a chat.
    #[tokio::test]
    async fn removing_a_source_deletes_only_what_it_owns() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let papers = dir.path().join("Papers");
        let later = dir.path().join("Papers 2024");
        std::fs::create_dir_all(papers.join("nested")).unwrap();
        std::fs::create_dir_all(&later).unwrap();
        let owned = |id: &str| HashMap::from([("space_id".to_string(), id.to_string())]);
        let chat = HashMap::from([(
            "uploaded_at".to_string(),
            "2026-10-01T09:00:00Z".to_string(),
        )]);
        // Added in a chat by an older version, which gave it the id of the
        // Library source selected then.
        let tagged_chat = HashMap::from([
            (
                "uploaded_at".to_string(),
                "2026-10-01T09:00:00Z".to_string(),
            ),
            ("space_id".to_string(), "papers".to_string()),
        ]);
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let files = [
            (papers.join("own.md"), "zebra", owned("papers")),
            (later.join("later.md"), "giraffe", owned("later")),
            (
                papers.join("nested").join("nested.md"),
                "pelican",
                owned("nested"),
            ),
            (papers.join("chat.md"), "walrus", chat),
            (papers.join("legacy.md"), "otter", HashMap::new()),
            (later.join("legacy.md"), "lynx", HashMap::new()),
            (uploads.join("tagged.md"), "heron", tagged_chat),
        ];
        for (path, word, metadata) in &files {
            let text = format!(
                "# Notes\n\nThe {word} was seen near the river at dawn. The {word} \
                 returned the next morning, and the field notes describe the {word} at length.\n"
            );
            std::fs::write(path, text).unwrap();
            engine
                .add_document_from_file(path, metadata.clone())
                .await
                .unwrap();
        }
        let names = |rows: Vec<crate::storage::DocumentSourceRow>| {
            let mut names: Vec<String> = rows
                .into_iter()
                .map(|row| {
                    let parent = Path::new(&row.source)
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    let name = Path::new(&row.source)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    format!("{parent}/{name}")
                })
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(engine.document_sources().await.unwrap()).len(), 7);

        let papers_path = papers.display().to_string();
        let deleted = engine
            .delete_library_source("papers", Some(papers_path.as_str()), &[])
            .await
            .unwrap();
        assert!(deleted >= 2, "its own file and the legacy file under it");
        assert_eq!(
            names(engine.document_sources().await.unwrap()),
            [
                "nested/nested.md",
                "papers 2024/later.md",
                "papers 2024/legacy.md",
                "papers/chat.md",
                "uploads/tagged.md",
            ]
        );
        // The upload the older version tagged with the folder is the chats' now.
        let tagged = normalize_source_path(&uploads.join("tagged.md"));
        assert!(engine
            .document_sources()
            .await
            .unwrap()
            .iter()
            .any(|row| row.source == tagged && row.space_id == CHAT_UPLOADS_MAIN));
        // The keyword index keeps exactly the same files.
        for (word, kept) in [
            ("zebra", false),
            ("otter", false),
            ("giraffe", true),
            ("pelican", true),
            ("walrus", true),
            ("lynx", true),
            ("heron", true),
        ] {
            let hits = engine.text_search.search(word, 5).unwrap();
            assert_eq!(!hits.is_empty(), kept, "{word}");
        }
        // A source is removed by its id: none, none removed.
        let later_path = later.display().to_string();
        assert!(engine
            .delete_library_source(" ", Some(later_path.as_str()), &[])
            .await
            .is_err());
    }

    #[test]
    fn chat_uploads_are_owned_by_where_they_were_added() {
        assert_eq!(
            chat_uploads_source(None).as_deref(),
            Some(CHAT_UPLOADS_MAIN)
        );
        assert_eq!(
            chat_uploads_source(Some("ws-1a2b")).as_deref(),
            Some("chat-uploads:ws-1a2b")
        );
        for bad in ["", "main", "ws'1", "ws\"1", "ws/1", "ws 1"] {
            assert_eq!(chat_uploads_source(Some(bad)), None, "{bad:?}");
        }
        assert!(is_chat_uploads(CHAT_UPLOADS_MAIN));
        assert!(is_chat_uploads("chat-uploads:ws-1a2b"));
        assert!(is_chat_uploads("chat-uploads"));
        assert!(!is_chat_uploads("folder-chat-uploads"));
    }

    /// Uploads not owned where they were added go to the main Ask: filed
    /// under a folder or under no source by older versions, under the single
    /// owner of an earlier build, or under a workspace that was deleted. A
    /// live workspace's uploads stay, and so do folder files.
    #[tokio::test]
    async fn stray_chat_uploads_are_handed_to_the_main_ask() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let uploaded = (
            "uploaded_at".to_string(),
            "2026-10-01T09:00:00Z".to_string(),
        );
        for (name, owner, upload) in [
            ("folder-tagged.md", "folder-papers", true),
            ("untagged.md", "", true),
            ("legacy.md", LEGACY_CHAT_UPLOADS, true),
            ("deleted.md", "chat-uploads:ws-gone", true),
            ("live.md", "chat-uploads:ws-live", true),
            ("main.md", CHAT_UPLOADS_MAIN, true),
            ("paper.md", "folder-papers", false),
        ] {
            let path = dir.path().join(name);
            std::fs::write(
                &path,
                format!("# {name}\n\nThe {name} notes describe the river, the bridge and the mill at dawn.\n"),
            )
            .unwrap();
            let mut metadata = HashMap::from([("space_id".to_string(), owner.to_string())]);
            if upload {
                metadata.insert(uploaded.0.clone(), uploaded.1.clone());
            }
            engine
                .add_document_from_file(&path, metadata)
                .await
                .unwrap();
        }
        let owners = |rows: Vec<crate::storage::DocumentSourceRow>| {
            let mut owners: Vec<(String, String)> = rows
                .into_iter()
                .map(|row| {
                    let name = Path::new(&row.source)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (name, row.space_id)
                })
                .collect();
            owners.sort();
            owners
        };
        let live = std::collections::HashSet::from(["ws-live".to_string()]);
        assert!(engine.hand_over_chat_uploads(Some(&live)).await.unwrap() >= 4);
        let main = CHAT_UPLOADS_MAIN.to_string();
        assert_eq!(
            owners(engine.document_sources().await.unwrap()),
            [
                ("deleted.md".to_string(), main.clone()),
                ("folder-tagged.md".to_string(), main.clone()),
                ("legacy.md".to_string(), main.clone()),
                ("live.md".to_string(), "chat-uploads:ws-live".to_string()),
                ("main.md".to_string(), main.clone()),
                ("paper.md".to_string(), "folder-papers".to_string()),
                ("untagged.md".to_string(), main.clone()),
            ]
        );
        // Nothing left to hand over; with the workspaces unknown, no
        // workspace's uploads are touched.
        assert_eq!(engine.hand_over_chat_uploads(Some(&live)).await.unwrap(), 0);
        assert_eq!(engine.hand_over_chat_uploads(None).await.unwrap(), 0);
    }

    /// Unowned chunks under another listed source's folder are that source's
    /// business: removing a folder that contains it leaves them.
    #[tokio::test]
    async fn unowned_files_under_another_listed_folder_stay() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let research = dir.path().join("Research");
        std::fs::create_dir_all(research.join("Papers")).unwrap();
        for (path, word) in [
            (research.join("notes.md"), "otter"),
            (research.join("Papers").join("paper.md"), "pelican"),
        ] {
            std::fs::write(
                &path,
                format!("# Notes\n\nThe {word} was seen near the river at dawn. The {word} returned the next morning, and the notes describe the {word}.\n"),
            )
            .unwrap();
            engine
                .add_document_from_file(&path, HashMap::new())
                .await
                .unwrap();
        }
        let papers = research.join("Papers").display().to_string();
        let research_path = research.display().to_string();
        engine
            .delete_library_source("research", Some(research_path.as_str()), &[papers])
            .await
            .unwrap();
        let left: Vec<String> = engine
            .document_sources()
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.source)
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].ends_with("paper.md"), "{left:?}");
        assert!(!engine.text_search.search("pelican", 5).unwrap().is_empty());
        assert!(engine.text_search.search("otter", 5).unwrap().is_empty());
    }

    #[test]
    fn unowned_files_match_a_folder_only_across_a_separator() {
        let papers = source_spellings(Path::new("/research/papers/"));
        assert!(
            papers.contains(&"/research/papers".to_string()),
            "{papers:?}"
        );
        assert!(lies_under("/research/papers", &papers), "the folder itself");
        assert!(lies_under("/research/papers/a.pdf", &papers));
        assert!(lies_under("/research/papers/deep/b.pdf", &papers));
        assert!(!lies_under("/research/papers 2024/a.pdf", &papers));
        assert!(!lies_under("/research/papersx.pdf", &papers));
        // Backslash spellings of older indexers, across their separator.
        let legacy = vec![r"C:\Research\Papers".to_string()];
        assert!(lies_under(r"C:\Research\Papers\a.pdf", &legacy));
        assert!(!lies_under(r"C:\Research\Papers 2024\a.pdf", &legacy));
        // No character is a wildcard.
        let odd = vec!["/data/50%_o'brien".to_string()];
        assert!(lies_under("/data/50%_o'brien/a.pdf", &odd));
        assert!(!lies_under("/data/50xyo'brien/a.pdf", &odd));
        assert!(added_in_chat(
            r#"{"uploaded_at":"2026-10-01T09:00:00Z","space_id":""}"#
        ));
        assert!(!added_in_chat(r#"{"title":"uploaded_at"}"#));
        assert!(!added_in_chat("not json"));
    }

    /// Every chunk records the format and the context it was indexed with;
    /// the context holds the document's opening, the stored text does not.
    #[tokio::test]
    async fn chunks_record_their_index_format_and_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let lease = dir.path().join("lease.md");
        std::fs::write(&lease, LEASE).unwrap();
        engine
            .add_document_from_file(&lease, HashMap::new())
            .await
            .unwrap();
        let chunks = engine.store.list_chunks(None, 100).await.unwrap();
        let rent = chunks
            .iter()
            .find(|c| c.text.contains("4,85,000"))
            .expect("the rent chunk");
        assert!(!rent.text.contains("Saffron"), "stored text is the passage");
        let metadata: HashMap<String, String> = serde_json::from_str(&rent.metadata_json).unwrap();
        assert_eq!(
            metadata.get(INDEX_FORMAT_KEY),
            Some(&INDEX_FORMAT.to_string())
        );
        // The opening is the start of the first chunk (here all of it).
        assert_eq!(
            metadata.get(CHUNK_CONTEXT_KEY).map(String::as_str),
            Some(
                "Document: \"lease\". Opening: Lessor: Saffron Estates Pvt. Ltd. \
                 Lessee: Tidewater Logistics Pvt. Ltd. Section: Rent. "
            )
        );
        // The first chunk carries the names too.
        let first = chunks.iter().find(|c| c.text.contains("Saffron")).unwrap();
        let metadata: HashMap<String, String> = serde_json::from_str(&first.metadata_json).unwrap();
        assert!(metadata[CHUNK_CONTEXT_KEY].starts_with("Document: \"lease\". Opening: Lessor:"));
        assert_eq!(
            metadata.get(CHUNKER_VERSION_KEY),
            Some(&CHUNKER_VERSION.to_string())
        );
        assert!(engine.stale_sources().await.unwrap().is_empty());
    }

    /// Documents indexed by an earlier version are listed as stale until they
    /// are indexed again; indexing one again replaces its chunks.
    #[tokio::test]
    async fn documents_of_an_older_index_format_are_stale_until_indexed_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let old = dir.path().join("lease.md");
        let new = dir.path().join("notes.md");
        std::fs::write(&old, LEASE).unwrap();
        std::fs::write(&new, "# Notes\n\nThe roof is repaired every spring.\n").unwrap();
        let space = HashMap::from([("space_id".to_string(), "s1".to_string())]);
        let prepared = engine
            .file_preparer()
            .unwrap()
            .prepare(&old, space.clone())
            .unwrap();
        let old_chunks = engine.commit_file_as_format_one(prepared).await.unwrap();
        engine
            .add_document_from_file(&new, space.clone())
            .await
            .unwrap();

        let stale = engine.stale_sources().await.unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].source, normalize_source_path(&old));
        assert_eq!(stale[0].space_id, "s1");
        assert_eq!(stale[0].chunks, old_chunks.len());

        engine.add_document_from_file(&old, space).await.unwrap();
        assert!(engine.stale_sources().await.unwrap().is_empty());
        let rows = engine.document_sources().await.unwrap();
        let lease = rows
            .iter()
            .find(|r| r.source == normalize_source_path(&old))
            .unwrap();
        assert_eq!(lease.chunks, old_chunks.len(), "replaced, not added");
    }

    /// Upgraded in place, a document's chunks get the contexts indexing it
    /// afresh gives them, keeping their ids and text.
    #[tokio::test]
    async fn an_upgrade_in_place_gives_the_contexts_of_a_fresh_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let lease = dir.path().join("lease.md");
        std::fs::write(&lease, LEASE).unwrap();
        let source = normalize_source_path(&lease);
        let contexts = |chunks: &[crate::storage::SearchHit]| -> Vec<(String, String)> {
            let mut out: Vec<(String, String)> = chunks
                .iter()
                .map(|c| {
                    let metadata: HashMap<String, String> =
                        serde_json::from_str(&c.metadata_json).unwrap();
                    (
                        c.text.clone(),
                        metadata.get(CHUNK_CONTEXT_KEY).cloned().unwrap_or_default(),
                    )
                })
                .collect();
            out.sort();
            out
        };
        engine
            .add_document_from_file(&lease, HashMap::new())
            .await
            .unwrap();
        let fresh = contexts(&engine.store.list_chunks(None, 100).await.unwrap());

        let prepared = engine
            .file_preparer()
            .unwrap()
            .prepare(&lease, HashMap::new())
            .unwrap();
        engine.commit_file_as_format_one(prepared).await.unwrap();
        let old = engine.store.list_chunks(None, 100).await.unwrap();
        assert!(contexts(&old).iter().all(|(_, context)| context.is_empty()));
        assert_eq!(engine.stale_sources().await.unwrap().len(), 1);

        assert!(engine.upgrade_in_place(&source).await.unwrap());
        let upgraded = engine.store.list_chunks(None, 100).await.unwrap();
        assert_eq!(contexts(&upgraded), fresh);
        let mut old_ids: Vec<_> = old.iter().map(|c| c.id.clone()).collect();
        let mut new_ids: Vec<_> = upgraded.iter().map(|c| c.id.clone()).collect();
        old_ids.sort();
        new_ids.sort();
        assert_eq!(old_ids, new_ids);
        assert!(engine.stale_sources().await.unwrap().is_empty());
        assert!(!engine.upgrade_in_place("no such source").await.unwrap());
    }

    /// A commit stores the new chunks before it removes the old ones. A
    /// failure while storing leaves the old chunks; a failure while removing
    /// them leaves both copies, written down, until the next commit or start
    /// removes the old ones.
    #[tokio::test]
    async fn a_replacement_never_leaves_a_file_without_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let lease = dir.path().join("lease.md");
        std::fs::write(&lease, LEASE).unwrap();
        let stored =
            |chunks: Vec<crate::storage::SearchHit>| -> std::collections::BTreeSet<String> {
                chunks.into_iter().map(|c| c.id).collect()
            };
        let as_set = |ids: &[Uuid]| -> std::collections::BTreeSet<String> {
            ids.iter().map(Uuid::to_string).collect()
        };
        let first = as_set(
            &engine
                .add_document_from_file(&lease, HashMap::new())
                .await
                .unwrap(),
        );
        assert!(!first.is_empty());

        // Storing fails: the old chunks are as they were.
        engine
            .fail_next_store
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(engine
            .add_document_from_file(&lease, HashMap::new())
            .await
            .is_err());
        assert_eq!(
            stored(engine.store.list_chunks(None, 100).await.unwrap()),
            first
        );

        // Removing the old chunks fails after the new ones are stored: both
        // copies stay, and the old ones are written down.
        engine
            .fail_replaced_removal
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let second = as_set(
            &engine
                .add_document_from_file(&lease, HashMap::new())
                .await
                .unwrap(),
        );
        assert!(second.is_disjoint(&first));
        let both: std::collections::BTreeSet<String> = first.union(&second).cloned().collect();
        assert_eq!(
            stored(engine.store.list_chunks(None, 100).await.unwrap()),
            both
        );
        assert_eq!(engine.text_search.count().unwrap(), both.len());
        let pending = engine.replaced_chunks().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].source, normalize_source_path(&lease));
        assert_eq!(
            pending[0]
                .previous_ids
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            first
        );

        // The next start (or commit) removes them.
        engine
            .fail_replaced_removal
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(engine.settle_replaced().await.unwrap(), first.len());
        assert_eq!(
            stored(engine.store.list_chunks(None, 100).await.unwrap()),
            second
        );
        assert_eq!(engine.text_search.count().unwrap(), second.len());
        assert!(!engine.replaced_chunks_path().exists());
        assert_eq!(engine.settle_replaced().await.unwrap(), 0);

        // A quit after the intent was written but before anything was
        // stored: the file still has only its previous chunks, which stay.
        assert!(engine.begin_replacement(
            &normalize_source_path(&lease),
            &second.iter().cloned().collect::<Vec<_>>()
        ));
        assert_eq!(engine.settle_replaced().await.unwrap(), 0);
        assert_eq!(
            stored(engine.store.list_chunks(None, 100).await.unwrap()),
            second
        );
        assert!(!engine.replaced_chunks_path().exists());

        // A quit after the new chunks were stored, before the record of the
        // removal: the intent written first removes the previous ones.
        let prepared = engine
            .file_preparer()
            .unwrap()
            .prepare(&lease, HashMap::new())
            .unwrap();
        let source = normalize_source_path(&lease);
        let before: Vec<String> = second.iter().cloned().collect();
        assert!(engine.begin_replacement(&source, &before));
        let third = as_set(&engine.store_prepared(prepared.document).await.unwrap());
        let both: std::collections::BTreeSet<String> = second.union(&third).cloned().collect();
        assert_eq!(
            stored(engine.store.list_chunks(None, 100).await.unwrap()),
            both
        );
        assert_eq!(engine.settle_replaced().await.unwrap(), second.len());
        assert_eq!(
            stored(engine.store.list_chunks(None, 100).await.unwrap()),
            third
        );
    }

    /// The chunks of version 1 are marked from the start of their metadata:
    /// a value holding `{"` (or ending in `{`) is left as it is.
    #[tokio::test]
    async fn marking_kept_chunks_leaves_values_as_they_are() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let lease = dir.path().join("lease.md");
        std::fs::write(&lease, LEASE).unwrap();
        let source = normalize_source_path(&lease);
        let odd = "a brace at the end {";
        let quoted = "{\"key\": \"value\"}";
        let metadata = HashMap::from([
            ("note".to_string(), odd.to_string()),
            ("raw".to_string(), quoted.to_string()),
        ]);
        let prepared = engine
            .file_preparer()
            .unwrap()
            .prepare(&lease, metadata)
            .unwrap();
        engine
            .commit_file_as_chunker_one(prepared, str::to_string)
            .await
            .unwrap();
        let marked = engine.keep_chunks_of_older_chunker(&source).await.unwrap();
        let chunks = engine.store.list_chunks(None, 100).await.unwrap();
        assert_eq!(marked as usize, chunks.len());
        for chunk in &chunks {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&chunk.metadata_json).expect("still JSON");
            assert_eq!(metadata.get("note").map(String::as_str), Some(odd));
            assert_eq!(metadata.get("raw").map(String::as_str), Some(quoted));
            assert_eq!(
                metadata.get(CHUNKS_KEPT_FROM_KEY).map(String::as_str),
                Some("1")
            );
            assert_eq!(
                metadata.get(CHUNKER_VERSION_KEY),
                Some(&CHUNKER_VERSION.to_string())
            );
        }
    }

    /// Chunks record the chunker version they were made with. Chunks of an
    /// older version are listed for re-chunking and are never upgraded in
    /// place (their text is what changed), unless kept on purpose: then only
    /// their contexts change, and their text and version stay.
    #[tokio::test]
    async fn chunks_of_an_older_chunker_are_rechunked_never_upgraded_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let lease = dir.path().join("lease.md");
        std::fs::write(&lease, LEASE).unwrap();
        let source = normalize_source_path(&lease);
        engine
            .add_document_from_file(&lease, HashMap::new())
            .await
            .unwrap();
        let chunks = engine.store.list_chunks(None, 100).await.unwrap();
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&chunk.metadata_json).unwrap();
            assert_eq!(
                metadata.get(CHUNKER_VERSION_KEY),
                Some(&CHUNKER_VERSION.to_string())
            );
        }
        assert!(engine.rechunk_sources().await.unwrap().is_empty());
        assert!(engine.outdated_sources().await.unwrap().is_empty());

        let prepared = engine
            .file_preparer()
            .unwrap()
            .prepare(&lease, HashMap::new())
            .unwrap();
        engine
            .commit_file_as_chunker_one(prepared, |text| format!("{text} (as chunked before)"))
            .await
            .unwrap();
        let rechunk = engine.rechunk_sources().await.unwrap();
        assert_eq!(rechunk.len(), 1);
        assert_eq!(rechunk[0].source, source);
        assert!(
            engine.stale_sources().await.unwrap().is_empty(),
            "the format is current"
        );
        assert_eq!(engine.outdated_sources().await.unwrap().len(), 1);
        assert!(
            !engine.upgrade_in_place(&source).await.unwrap(),
            "never upgraded in place"
        );

        // Kept on purpose: the text and the version stay, the contexts move on.
        let before = engine.store.list_chunks(None, 100).await.unwrap();
        assert!(engine.upgrade_format_keeping_chunks(&source).await.unwrap());
        let kept = engine.store.list_chunks(None, 100).await.unwrap();
        let texts = |chunks: &[crate::storage::SearchHit]| -> Vec<String> {
            let mut texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
            texts.sort();
            texts
        };
        assert_eq!(texts(&kept), texts(&before));
        assert!(kept.iter().all(|c| c.text.ends_with("(as chunked before)")));
        assert_eq!(engine.rechunk_sources().await.unwrap().len(), 1);

        // Recorded as kept: no longer listed, so the scan over every chunk's
        // metadata latches off; the text, ids and vectors stay.
        let marked = engine.keep_chunks_of_older_chunker(&source).await.unwrap();
        assert_eq!(marked as usize, kept.len());
        assert!(engine.rechunk_sources().await.unwrap().is_empty());
        assert!(engine.rechunk_latched());
        let recorded = engine.store.list_chunks(None, 100).await.unwrap();
        assert_eq!(texts(&recorded), texts(&before));
        for chunk in &recorded {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&chunk.metadata_json).unwrap();
            assert_eq!(
                metadata.get(CHUNKER_VERSION_KEY),
                Some(&CHUNKER_VERSION.to_string())
            );
            assert_eq!(
                metadata.get(CHUNKS_KEPT_FROM_KEY).map(String::as_str),
                Some("1")
            );
            assert_eq!(
                metadata.get(INDEX_FORMAT_KEY),
                Some(&INDEX_FORMAT.to_string()),
                "the rest of the metadata is kept"
            );
        }
        assert_eq!(
            engine.keep_chunks_of_older_chunker(&source).await.unwrap(),
            0,
            "nothing older is left to mark"
        );

        // Indexed again from the file: the text of the chunker now.
        engine
            .add_document_from_file(&lease, HashMap::new())
            .await
            .unwrap();
        assert!(engine.rechunk_sources().await.unwrap().is_empty());
        let fresh = engine.store.list_chunks(None, 100).await.unwrap();
        assert!(fresh
            .iter()
            .all(|c| !c.text.contains("(as chunked before)")));
        assert!(fresh
            .iter()
            .all(|c| !c.metadata_json.contains(CHUNKS_KEPT_FROM_KEY)));
    }

    /// No context names a file's folders: not for plain text (window chunks),
    /// not for structure chunks.
    #[tokio::test]
    async fn contexts_name_the_file_never_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let folder = dir.path().join("Private Client Files");
        std::fs::create_dir_all(&folder).unwrap();
        let notes = folder.join("notes.txt");
        std::fs::write(
            &notes,
            "Quiet hours begin at ten in the evening. ".repeat(80),
        )
        .unwrap();
        let lease = folder.join("lease.md");
        std::fs::write(&lease, LEASE).unwrap();
        for file in [&notes, &lease] {
            engine
                .add_document_from_file(file, HashMap::new())
                .await
                .unwrap();
        }
        let chunks = engine.store.list_chunks(None, 100).await.unwrap();
        assert!(chunks.iter().any(|c| c.source.ends_with("notes.txt")));
        for chunk in &chunks {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&chunk.metadata_json).unwrap();
            let context = metadata.get(CHUNK_CONTEXT_KEY).unwrap();
            assert!(!context.contains("Private Client Files"), "{context}");
            assert!(
                !context.contains(&*dir.path().to_string_lossy()),
                "{context}"
            );
        }
        let window = chunks
            .iter()
            .find(|c| c.source.ends_with("notes.txt"))
            .unwrap();
        let metadata: HashMap<String, String> =
            serde_json::from_str(&window.metadata_json).unwrap();
        assert!(metadata[CHUNK_CONTEXT_KEY].contains("Source: notes.txt."));
        assert_eq!(source_label("calendar://task/7"), "calendar://task/7");
        assert_eq!(source_label("C:\\Users\\a\\b.txt"), "b.txt");
        assert_eq!(source_label("/home/a/b.txt"), "b.txt");
    }

    /// The cross-encoder reads a passage with the context it was embedded
    /// with; a chunk of format 1 with its document's title and section.
    #[test]
    fn the_reranker_reads_each_passage_with_its_context() {
        let mut current = result(0.5, "text");
        current.metadata.insert(
            CHUNK_CONTEXT_KEY.to_string(),
            "Document: \"lease\". Opening: Lessor: Saffron Estates. ".to_string(),
        );
        assert_eq!(
            rerank_context(&current),
            "Document: \"lease\". Opening: Lessor: Saffron Estates. "
        );

        let mut older = result(0.5, "text");
        older.citation.title = "lease".to_string();
        older
            .metadata
            .insert("heading".to_string(), "1. Rent".to_string());
        assert_eq!(
            rerank_context(&older),
            "Document: \"lease\". Section: 1. Rent. "
        );
    }

    /// Rebuilding the keyword index keeps each chunk's context: a party named
    /// only in the opening still finds the clause.
    #[tokio::test]
    async fn the_rebuilt_keyword_index_keeps_the_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let lease = dir.path().join("lease.md");
        std::fs::write(&lease, LEASE).unwrap();
        engine
            .add_document_from_file(&lease, HashMap::new())
            .await
            .unwrap();
        let rent_id = engine
            .store
            .list_chunks(None, 100)
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.text.contains("4,85,000"))
            .unwrap()
            .id;
        let finds_rent = |engine: &RAGEngine| {
            engine
                .text_search
                .search("Saffron", 10)
                .unwrap()
                .iter()
                .any(|(id, _)| *id == rent_id)
        };
        assert!(finds_rent(&engine));
        engine.rebuild_text_index().await.unwrap();
        assert!(finds_rent(&engine));
    }

    #[tokio::test]
    async fn reference_entries_answer_only_queries_about_citations() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_with_paper(dir.path()).await;
        let kinds = |results: &[ComprehensiveResult]| -> Vec<String> {
            results
                .iter()
                .map(|r| r.metadata.get("unit_kind").cloned().unwrap_or_default())
                .collect()
        };

        let content = Box::pin(engine.search_comprehensive(
            "delta rule parallelized over sequence length",
            5,
            None,
        ))
        .await
        .unwrap();
        assert!(!content.is_empty());
        assert!(
            !kinds(&content).iter().any(|k| k == "reference_entry"),
            "{:?}",
            kinds(&content)
        );
        assert!(
            content[0].snippet.contains("parallelized over"),
            "{}",
            content[0].snippet
        );

        let citations = Box::pin(engine.search_comprehensive(
            "which paper proposed the delta rule over sequence length",
            5,
            None,
        ))
        .await
        .unwrap();
        assert!(
            kinds(&citations).iter().any(|k| k == "reference_entry"),
            "{:?}",
            kinds(&citations)
        );
    }

    #[test]
    fn only_window_chunks_are_widened_with_neighbours() {
        let mut window = HashMap::new();
        window.insert("chunk_index".to_string(), "4".to_string());
        assert!(wants_neighbor_expansion(&window));
        let mut unit = window.clone();
        unit.insert("unit_kind".to_string(), "reference_entry".to_string());
        assert!(!wants_neighbor_expansion(&unit));
    }

    #[test]
    fn paged_chunk_metadata_round_trips_to_page_label() {
        let mut meta = HashMap::new();
        insert_page_metadata(&mut meta, Some(7), None);
        assert_eq!(meta.get("page").map(String::as_str), Some("7"));
        assert_eq!(meta.get("page_start").map(String::as_str), Some("7"));
        assert_eq!(meta.get("page_end").map(String::as_str), Some("7"));
        assert_eq!(page_numbers_from_metadata(&meta).as_deref(), Some("7"));
    }

    #[test]
    fn a_chunk_running_onto_a_later_page_records_the_range() {
        let mut meta = HashMap::new();
        insert_page_metadata(&mut meta, Some(2), Some(3));
        assert_eq!(meta.get("page").map(String::as_str), Some("2"));
        assert_eq!(meta.get("page_start").map(String::as_str), Some("2"));
        assert_eq!(meta.get("page_end").map(String::as_str), Some("3"));
        assert_eq!(page_numbers_from_metadata(&meta).as_deref(), Some("2-3"));
        // An end before the start is no range.
        let mut meta = HashMap::new();
        insert_page_metadata(&mut meta, Some(4), Some(1));
        assert_eq!(page_numbers_from_metadata(&meta).as_deref(), Some("4"));
    }

    #[test]
    fn unpaged_chunk_has_no_page_label() {
        let mut meta = HashMap::new();
        insert_page_metadata(&mut meta, None, None);
        assert!(meta.is_empty());
        assert_eq!(page_numbers_from_metadata(&meta), None);
    }

    #[test]
    fn page_range_and_legacy_page_key() {
        let mut meta = HashMap::new();
        meta.insert("page_start".to_string(), "3".to_string());
        meta.insert("page_end".to_string(), "5".to_string());
        assert_eq!(page_numbers_from_metadata(&meta).as_deref(), Some("3-5"));

        let mut legacy = HashMap::new();
        legacy.insert("page".to_string(), "12".to_string());
        assert_eq!(page_numbers_from_metadata(&legacy).as_deref(), Some("12"));

        let mut blank = HashMap::new();
        blank.insert("page".to_string(), "  ".to_string());
        assert_eq!(page_numbers_from_metadata(&blank), None);
    }

    /// `relative` (folders and file, with their case) created under `dir`
    /// with `text`, below `dir` as the file system spells it: a Windows
    /// runner's temporary folder is often a short `8.3` name, which
    /// resolving a path expands.
    #[cfg(windows)]
    fn mixed_case_file(dir: &Path, relative: &str, text: &str) -> std::path::PathBuf {
        let root = canonical_path(&std::fs::canonicalize(dir).unwrap());
        let file = root.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        file
    }

    /// A file indexed under a lowercased spelling of its path keeps the
    /// lowercase identity (`source`, `file_path`) and records the case its
    /// folders and name have on disk (`display_path`).
    #[cfg(windows)]
    #[tokio::test]
    async fn a_lowercased_path_keeps_its_identity_and_records_the_case_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let file = mixed_case_file(dir.path(), "Tmp\\Mixed\\SPANN - X.md", LEASE);
        let spelled = file.display().to_string();
        let lowered = std::path::PathBuf::from(spelled.to_lowercase());
        let identity = normalize_source_path(&file);
        assert_eq!(identity, identity.to_lowercase());
        assert_eq!(normalize_source_path(&lowered), identity);
        assert_eq!(
            on_disk_spelling(&lowered).as_deref(),
            Some(spelled.as_str())
        );

        let prepared = engine
            .file_preparer()
            .unwrap()
            .prepare(&lowered, HashMap::new())
            .unwrap();
        assert_eq!(prepared.source, identity);
        assert!(!prepared.document.records.is_empty());
        for record in &prepared.document.records {
            assert_eq!(record.source, identity);
            let metadata: HashMap<String, String> =
                serde_json::from_str(&record.metadata_json).unwrap();
            assert_eq!(metadata["file_path"], identity);
            assert_eq!(metadata[DISPLAY_PATH_KEY], spelled);
        }
        engine.commit_file(prepared).await.unwrap();
        let shown = engine.document_names().await.unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!(
            shown.values().next().and_then(|n| n.display_path.as_ref()),
            Some(&spelled)
        );
    }

    /// Only the names are read from a chunk's metadata; blank ones are absent.
    #[test]
    fn document_names_skip_other_metadata() {
        let names: DocumentNames = serde_json::from_str(
            r#"{"display_path":"C:\\Papers\\A.pdf","filename":"  ","bboxes":"[{\"page\":1}]","title":"A"}"#,
        )
        .unwrap();
        assert_eq!(names.display_path.as_deref(), Some("C:\\Papers\\A.pdf"));
        assert_eq!(names.filename, None);
        let none: DocumentNames = serde_json::from_str(r#"{"title":"A"}"#).unwrap();
        assert_eq!(none, DocumentNames::default());
    }

    /// A file that cannot be resolved gives no spelling on Windows (its
    /// display path then falls back to the caller's spelling).
    #[cfg(windows)]
    #[test]
    fn an_unresolvable_file_has_no_spelling_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let file = mixed_case_file(dir.path(), "Papers\\A.md", "# A\n");
        assert!(on_disk_spelling(&file.with_file_name("Gone.md")).is_none());
        assert!(on_disk_spelling(&file).is_some());
    }

    /// Elsewhere a path keeps its case, so the caller's path is its spelling.
    #[cfg(not(windows))]
    #[test]
    fn a_path_is_its_own_spelling_off_windows() {
        let path = Path::new("/home/v/Research/./SPANN - X.pdf");
        assert_eq!(
            on_disk_spelling(path).as_deref(),
            Some("/home/v/Research/SPANN - X.pdf")
        );
    }
}

#[cfg(test)]
mod lazy_embedder_tests {
    use super::*;

    /// An installed E5 model is located at startup and loaded only by the first
    /// embedding; a model that cannot load turns search setup back on.
    #[tokio::test]
    async fn the_embedder_loads_on_first_use_not_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let e5 = dir.path().join("models").join("multilingual-e5-base");
        std::fs::create_dir_all(&e5).unwrap();
        // Present but not a model: loading it fails, which shows when it is tried.
        std::fs::write(e5.join("model_O4.onnx"), b"not an onnx model").unwrap();
        std::fs::write(e5.join("tokenizer.json"), b"{}").unwrap();
        let mut config = RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = true;
        config.embedding.dimension = 768;

        let engine = RAGEngine::new(config).await.unwrap();
        let handle = engine.embedder_handle();
        assert!(
            engine.has_search_models(),
            "installed models count as set up"
        );
        assert!(!handle.is_loaded(), "opening the engine loads nothing");
        assert_eq!(engine.embeddings().unwrap().dimension(), 768);

        // Nothing to embed (text too short to chunk): no load either.
        assert!(engine
            .embeddings()
            .unwrap()
            .embed_documents(&[])
            .unwrap()
            .is_empty());
        assert!(handle.available() && !handle.is_loaded());

        // The first real embedding tries to load it.
        assert!(engine
            .embeddings()
            .unwrap()
            .embed_query("tide pools")
            .is_err());
        assert!(!handle.available());
        assert!(
            !engine.has_search_models(),
            "a model that cannot load offers setup again"
        );
    }
}

#[cfg(test)]
mod neighbor_expansion_tests {
    use super::*;
    use crate::storage::SearchHit;

    fn hit(chunk_index: u32, text: &str, page: Option<usize>) -> SearchHit {
        let mut meta = HashMap::new();
        insert_page_metadata(&mut meta, page, None);
        SearchHit {
            id: format!("c{chunk_index}"),
            doc_id: "d1".to_string(),
            chunk_index,
            text: text.to_string(),
            title: "notes.txt".to_string(),
            source: "c:/notes.txt".to_string(),
            heading: String::new(),
            space_id: "s1".to_string(),
            metadata_json: serde_json::to_string(&meta).unwrap(),
            citation_json: "{}".to_string(),
            score: 0.0,
        }
    }

    fn parts_meta(parts: &SnippetParts) -> HashMap<String, String> {
        let mut meta = HashMap::new();
        meta.insert(
            SNIPPET_PARTS_KEY.to_string(),
            serde_json::to_string(parts).unwrap(),
        );
        meta
    }

    #[test]
    fn the_matched_chunk_and_its_neighbours_are_recorded_where_they_lie() {
        // Given out of order, as a store may return them; the chunk itself is skipped.
        let neighbors = [
            hit(8, "The ferry leaves at dawn.", Some(5)),
            hit(7, "duplicate of the core", Some(4)),
            hit(6, "東京の港は朝に開く。", Some(3)),
        ];
        let core = "The lighthouse keeper saw a comet.";
        let (text, parts) = join_neighbors(core, 7, &neighbors);
        assert_eq!(
            text,
            "東京の港は朝に開く。\nThe lighthouse keeper saw a comet.\nThe ferry leaves at dawn."
        );
        assert_eq!(&text[parts.matched.0..parts.matched.1], core);
        assert_eq!(parts.before.len(), 1);
        assert_eq!(parts.after.len(), 1);
        let before = parts.before[0];
        assert_eq!(&text[before.start..before.end], "東京の港は朝に開く。");
        assert_eq!(before.pages, Some((3, 3)));
        let after = parts.after[0];
        assert_eq!(&text[after.start..after.end], "The ferry leaves at dawn.");
        assert_eq!(after.pages, Some((5, 5)));
        // The record survives the metadata it travels in.
        assert_eq!(SnippetParts::of(&text, &parts_meta(&parts)), Some(parts));

        // The first chunk has nothing before it; an unpaged neighbour has no pages.
        let (text, parts) = join_neighbors(core, 0, &[hit(1, "Next chunk.", None)]);
        assert_eq!(parts.matched, (0, core.len()));
        assert!(parts.before.is_empty());
        assert_eq!(parts.after[0].pages, None);
        assert_eq!(
            &text[parts.after[0].start..parts.after[0].end],
            "Next chunk."
        );
    }

    #[test]
    fn the_windows_overlap_is_not_repeated() {
        let core = "東京の港は朝に開く。The lighthouse keeper saw a comet over the cliffs.";
        // The chunk before ends with the core's start, the chunk after starts with its end.
        let before = format!("Spring tide tables. {}", &core[..40]);
        let after = format!("{} The ferry leaves at dawn.", &core[core.len() - 20..]);
        let (text, parts) = join_neighbors(core, 5, &[hit(4, &before, None), hit(6, &after, None)]);
        assert_eq!(
            &text[parts.before[0].start..parts.before[0].end],
            "Spring tide tables. "
        );
        assert_eq!(&text[parts.matched.0..parts.matched.1], core);
        assert_eq!(
            &text[parts.after[0].start..parts.after[0].end],
            " The ferry leaves at dawn."
        );
        assert_eq!(text.matches("lighthouse").count(), 1);
        assert_eq!(text.matches("cliffs").count(), 1);

        // A short coincidence is not an overlap.
        assert_eq!(overlap_len("It was the", "the cat sat"), 0);
        // In bytes, on char boundaries; a whole neighbour can be the overlap.
        assert_eq!(
            overlap_len(
                "東京の港は朝に開く。東京の港",
                "東京の港は朝に開く。東京の港は"
            ),
            42
        );
        assert_eq!(
            overlap_len(
                "x 東京の港は朝に開く。東京の港",
                "東京の港は朝に開く。東京の港 y"
            ),
            42
        );
        let (text, parts) = join_neighbors(core, 5, &[hit(4, &core[..40], None)]);
        assert_eq!(parts.before[0].start, parts.before[0].end);
        assert_eq!(text, core);
        assert_eq!(SnippetParts::of(&text, &parts_meta(&parts)), Some(parts));
    }

    #[test]
    fn parts_that_do_not_describe_the_snippet_are_ignored() {
        let snippet = "東京\nabc";
        let at = |matched: (usize, usize)| SnippetParts {
            matched,
            ..SnippetParts::default()
        };
        assert!(SnippetParts::of(snippet, &HashMap::new()).is_none());
        assert!(SnippetParts::of(snippet, &parts_meta(&at((7, 10)))).is_some());
        // Inside a character, past the end, reversed.
        assert!(SnippetParts::of(snippet, &parts_meta(&at((1, 10)))).is_none());
        assert!(SnippetParts::of(snippet, &parts_meta(&at((7, 11)))).is_none());
        assert!(SnippetParts::of(snippet, &parts_meta(&at((8, 7)))).is_none());
        // A neighbour overlapping the matched chunk.
        let overlapping = SnippetParts {
            matched: (3, 10),
            before: vec![NeighborPart {
                start: 0,
                end: 6,
                pages: None,
            }],
            after: Vec::new(),
        };
        assert!(SnippetParts::of(snippet, &parts_meta(&overlapping)).is_none());
        let mut garbled = HashMap::new();
        garbled.insert(SNIPPET_PARTS_KEY.to_string(), "{not json".to_string());
        assert!(SnippetParts::of(snippet, &garbled).is_none());
    }

    #[test]
    fn page_ranges_come_from_the_chunk_metadata() {
        let meta = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            page_range_from_metadata(&meta(&[("page_start", "4"), ("page_end", "2")])),
            Some((2, 4))
        );
        assert_eq!(
            page_range_from_metadata(&meta(&[("page_end", "6")])),
            Some((6, 6))
        );
        assert_eq!(
            page_range_from_metadata(&meta(&[("page", " 9 ")])),
            Some((9, 9))
        );
        assert_eq!(page_range_from_metadata(&meta(&[("page", "x")])), None);
        assert_eq!(page_range_from_metadata(&HashMap::new()), None);
    }
}
