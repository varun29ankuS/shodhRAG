//! Building, storing and serving the citation graph.
//!
//! A build ([`CitationService::build`]):
//! 1. scans each library PDF (parsing it only when its size or modification time changed
//!    since the stored scan);
//! 2. matches papers to OpenAlex works through a [`Resolver`] — over the network only when
//!    the caller passes a networked resolver (the app does so only when the user's privacy
//!    policy allows the web); otherwise only answers already cached on this computer are
//!    read;
//! 3. assembles the graph ([`super::build::assemble`]) with the methods and datasets of the
//!    papers' accepted Result statements;
//! 4. writes the difference to the statement store: unchanged statements are left alone, a
//!    changed node supersedes its current statement (so a reference that disappeared also
//!    disappears from `cites`, with history kept), and nodes no longer in the graph are
//!    forgotten. A rebuild with nothing new writes nothing;
//! 5. replaces the in-memory snapshot the tools, pages and search ranker read.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use shodh_ontology::{RawValue, Statement};

use super::build::{assemble, DesiredStatement, ScanInput, GRAPH_EXTRACTOR};
use super::graph::{CitationEvidence, ConceptNode, GraphParts, GraphSize, PaperGraph, PaperNode};
use super::identity::read_pdf_info;
use super::resolve::{
    identifier_match, strict_match, Halt, Lookup, LookupStats, MatchVerdict, OpenAlexWork,
    Resolver, WorkQuery, WorkRequest,
};
use super::scan::{scan_document, PaperScan, SCAN_VERSION};
use super::segment::RejectReason;
use super::text::titles_agree;
use crate::processing::pdf_layout::parse_pdf_layout;
use crate::research::db::ResearchDb;
use crate::research::results::{ResultService, ResultStatus};
use crate::research::{
    blocking, canonical_file, file_name, path_key, ResearchError, ResearchResult,
};
use crate::search::graph_fusion::SourceRanker;
use crate::statements::{
    PutIntent, PutOutcome, Scope, StatementError, StatementQuery, StatementStore, StoredStatement,
};

/// Classes the graph writes.
pub const GRAPH_CLASSES: [&str; 4] = ["Paper", "Author", "Venue", "Method"];
/// Most library files ranked for one search.
const RANKED_FILES: usize = 20;
/// `citation_graph_state` keys.
const STATE_REPORT: &str = "report";
const STATE_SOURCES: &str = "sources";
const STATE_EVIDENCE: &str = "evidence";

/// The shared slot holding the current snapshot. The search ranker and the service read
/// the same slot, so a build is visible to search at once.
pub type GraphSlot = Arc<RwLock<Option<Arc<PaperGraph>>>>;

/// The search ranker over a graph slot.
pub struct GraphRanker {
    slot: GraphSlot,
}

impl GraphRanker {
    pub fn new(slot: GraphSlot) -> Self {
        Self { slot }
    }
}

impl SourceRanker for GraphRanker {
    fn rank_sources(&self, query: &str) -> Option<Vec<String>> {
        let graph = self.slot.read().clone()?;
        graph.rank_library_files(query, RANKED_FILES)
    }
}

/// Progress of a build, for the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
pub enum BuildProgress {
    Scanning {
        done: usize,
        total: usize,
        file: String,
    },
    Resolving {
        done: usize,
        total: usize,
    },
    Writing {
        done: usize,
        total: usize,
    },
}

/// A file that could not be scanned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedFile {
    pub file_path: String,
    pub reason: String,
}

/// What a build did; the last one is kept in `shodh.db`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildReport {
    pub files: usize,
    pub parsed: usize,
    pub reused_scans: usize,
    pub failed: Vec<FailedFile>,
    pub references: usize,
    pub identifiable_references: usize,
    pub unidentified_references: usize,
    pub rejected_blocks: BTreeMap<String, usize>,
    /// Library papers matched to an OpenAlex work.
    pub library_resolved: usize,
    /// References matched to an OpenAlex work.
    pub references_resolved: usize,
    pub references_linked_to_library: usize,
    /// Whether lookups could go to the network in this build.
    pub online: bool,
    pub lookups: Option<LookupStats>,
    /// Why lookups stopped early, if they did.
    pub halted: Option<String>,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    /// Statements the store refused (conflicts with statements made elsewhere, or invalid).
    pub refused: usize,
    pub size: GraphSize,
    pub built_at: chrono::DateTime<chrono::Utc>,
}

/// The citation graph over the statement store and `shodh.db`.
pub struct CitationService {
    store: Arc<StatementStore>,
    db: Arc<ResearchDb>,
    slot: GraphSlot,
    build_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for CitationService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CitationService").finish_non_exhaustive()
    }
}

fn file_fingerprint(path: &str) -> ResearchResult<String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| ResearchError::Pdf(format!("{} could not be read: {e}", file_name(path))))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Ok(format!("{SCAN_VERSION}|{}|{modified}", meta.len()))
}

fn is_graph_statement(stored: &StoredStatement) -> bool {
    stored
        .statement
        .provenance
        .as_ref()
        .is_some_and(|p| p.extractor.version.starts_with("citation-graph"))
}

fn text_value(statement: &Statement, key: &str) -> Option<String> {
    match statement.properties.get(key)? {
        RawValue::Text(t) => Some(t.clone()),
        RawValue::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

fn integer_value(statement: &Statement, key: &str) -> Option<i64> {
    match statement.properties.get(key)? {
        RawValue::Integer(i) => Some(*i),
        RawValue::Text(t) => t.parse().ok(),
        _ => None,
    }
}

fn entity_values(statement: &Statement, key: &str) -> Vec<String> {
    match statement.properties.get(key) {
        Some(RawValue::Entity(e)) => vec![e.id.clone()],
        Some(RawValue::List(items)) => items
            .iter()
            .filter_map(|v| match v {
                RawValue::Entity(e) => Some(e.id.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

impl CitationService {
    pub fn new(store: Arc<StatementStore>, db: Arc<ResearchDb>, slot: GraphSlot) -> Self {
        Self {
            store,
            db,
            slot,
            build_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// The slot the search ranker reads.
    pub fn slot(&self) -> GraphSlot {
        self.slot.clone()
    }

    /// The last build report, if the graph was ever built.
    pub async fn report(&self) -> ResearchResult<Option<BuildReport>> {
        let db = self.db.clone();
        Ok(blocking(move || db.graph_state(STATE_REPORT))
            .await?
            .and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// The current graph: the snapshot, or the graph read from the statement store when no
    /// snapshot exists yet (an empty graph when it was never built).
    pub async fn graph(&self, results: &ResultService) -> ResearchResult<Arc<PaperGraph>> {
        if let Some(g) = self.slot.read().clone() {
            return Ok(g);
        }
        let graph = Arc::new(self.load(results).await?);
        *self.slot.write() = Some(graph.clone());
        Ok(graph)
    }

    fn version_of(&self, class: &str) -> semver::Version {
        let ontology = self.store.ontology();
        ontology
            .class(class)
            .and_then(|c| ontology.source(&c.source))
            .map(|s| s.version.clone())
            .unwrap_or_else(|| ontology.version().clone())
    }

    /// The scan of one file: the stored one when the file is unchanged, else a new one.
    async fn scan(&self, path: &str) -> ResearchResult<(PaperScan, bool)> {
        let fingerprint = {
            let p = path.to_string();
            blocking(move || file_fingerprint(&p)).await?
        };
        let key = path_key(path);
        let db = self.db.clone();
        let stored_key = key.clone();
        if let Some((stored, json)) = blocking(move || db.citation_scan(&stored_key)).await? {
            if stored == fingerprint {
                if let Ok(scan) = serde_json::from_str::<PaperScan>(&json) {
                    if scan.version == SCAN_VERSION {
                        return Ok((scan, false));
                    }
                }
            }
        }
        let read_path = path.to_string();
        let scan = blocking(move || {
            let bytes = std::fs::read(&read_path).map_err(|e| {
                ResearchError::Pdf(format!("{} could not be read: {e}", file_name(&read_path)))
            })?;
            let doc = parse_pdf_layout(&bytes)
                .map_err(|e| ResearchError::Pdf(format!("The PDF could not be parsed: {e}")))?;
            let info = read_pdf_info(&bytes);
            Ok(scan_document(&doc, &info, &read_path))
        })
        .await?;
        let json = serde_json::to_string(&scan)
            .map_err(|e| ResearchError::Database(format!("scan not encodable: {e}")))?;
        let db = self.db.clone();
        blocking(move || db.put_citation_scan(&key, &fingerprint, &json)).await?;
        Ok((scan, true))
    }

    /// Builds the graph of `files` (indexed PDFs). See the module documentation.
    pub async fn build(
        &self,
        files: &[String],
        results: &ResultService,
        resolver: &Resolver,
        progress: &(dyn Fn(BuildProgress) + Send + Sync),
    ) -> ResearchResult<BuildReport> {
        let _guard = self.build_lock.lock().await;
        let mut paths: Vec<String> = files
            .iter()
            .map(|f| canonical_file(f))
            .filter(|f| f.to_ascii_lowercase().ends_with(".pdf"))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // One scan per file whatever the spelling.
        let mut seen = HashSet::new();
        paths.retain(|p| seen.insert(path_key(p)));

        let mut scans: Vec<PaperScan> = Vec::new();
        let mut failed = Vec::new();
        let (mut parsed, mut reused) = (0, 0);
        for (done, path) in paths.iter().enumerate() {
            progress(BuildProgress::Scanning {
                done,
                total: paths.len(),
                file: file_name(path),
            });
            match self.scan(path).await {
                Ok((scan, fresh)) => {
                    if fresh {
                        parsed += 1;
                    } else {
                        reused += 1;
                    }
                    scans.push(scan);
                }
                Err(e) => failed.push(FailedFile {
                    file_path: path.clone(),
                    reason: e.to_string(),
                }),
            }
        }
        let keep: HashSet<String> = paths.iter().map(|p| path_key(p)).collect();
        let db = self.db.clone();
        blocking(move || db.retain_citation_scans(&keep).map(|_| ())).await?;
        let counts = ScanCounts {
            files: paths.len(),
            parsed,
            reused,
            failed,
        };
        self.build_from_scans(scans, counts, results, resolver, progress)
            .await
    }

    /// The rest of a build once the files are scanned.
    pub(crate) async fn build_from_scans(
        &self,
        scans: Vec<PaperScan>,
        counts: ScanCounts,
        results: &ResultService,
        resolver: &Resolver,
        progress: &(dyn Fn(BuildProgress) + Send + Sync),
    ) -> ResearchResult<BuildReport> {
        let paths: Vec<String> = scans.iter().map(|s| s.file_path.clone()).collect();
        let mut inputs = self.resolve(scans, resolver, progress).await?;
        for input in &mut inputs {
            let paper = results.list(&input.scan.file_path).await?;
            for r in paper
                .results
                .iter()
                .filter(|r| r.status == ResultStatus::Accepted)
            {
                if !input
                    .result_methods
                    .iter()
                    .any(|(id, _)| id == &r.method_id)
                {
                    input
                        .result_methods
                        .push((r.method_id.clone(), r.method.clone()));
                }
                if !input
                    .result_datasets
                    .iter()
                    .any(|(id, _)| id == &r.dataset_id)
                {
                    input
                        .result_datasets
                        .push((r.dataset_id.clone(), r.dataset.clone()));
                }
            }
        }

        let assembled = assemble(&inputs);
        let previous_sources = self.previous_sources().await?;
        let mut sources: BTreeSet<String> = assembled
            .statements
            .iter()
            .map(|s| s.source.clone())
            .collect();
        sources.extend(paths.iter().cloned());
        let written = self
            .write(&assembled.statements, &previous_sources, progress)
            .await?;

        let mut parts = assembled.parts.clone();
        for paper in &mut parts.papers {
            paper.statement_id = written.ids.get(&("Paper", paper.id.clone())).cloned();
        }
        let evidence: Vec<(String, String, CitationEvidence)> = parts
            .cites
            .iter()
            .filter_map(|(a, b, e)| e.clone().map(|e| (a.clone(), b.clone(), e)))
            .collect();
        let graph = Arc::new(PaperGraph::new(parts));
        let size = graph.size();
        *self.slot.write() = Some(graph);

        let mut rejected_blocks: BTreeMap<String, usize> = BTreeMap::new();
        for input in &inputs {
            for (reason, n) in &input.scan.rejected {
                *rejected_blocks
                    .entry(reject_label(*reason).to_string())
                    .or_insert(0) += n;
            }
        }
        let references: usize = inputs.iter().map(|i| i.scan.references.len()).sum();
        let identifiable: usize = inputs.iter().map(|i| i.scan.identifiable().count()).sum();
        let report = BuildReport {
            files: counts.files,
            parsed: counts.parsed,
            reused_scans: counts.reused,
            failed: counts.failed,
            references,
            identifiable_references: identifiable,
            unidentified_references: assembled.unidentified,
            rejected_blocks,
            library_resolved: inputs.iter().filter(|i| i.work.is_some()).count(),
            references_resolved: inputs.iter().map(|i| i.reference_works.len()).sum(),
            references_linked_to_library: assembled.local_title_links,
            online: resolver.is_online(),
            lookups: Some(resolver.stats().await),
            halted: resolver.halted().await.and_then(|h| match h {
                Halt::Offline => None,
                Halt::Budget => Some("the per-build lookup budget was used up".to_string()),
                Halt::RateLimited => Some("OpenAlex asked Shodh to slow down (429)".to_string()),
                Halt::Unreachable(e) => Some(format!("OpenAlex could not be reached: {e}")),
            }),
            added: written.added,
            updated: written.updated,
            unchanged: written.unchanged,
            removed: written.removed,
            refused: written.refused,
            size,
            built_at: self.store.now(),
        };
        let report_json = serde_json::to_string(&report)
            .map_err(|e| ResearchError::Database(format!("report not encodable: {e}")))?;
        let sources_json = serde_json::to_string(&sources)
            .map_err(|e| ResearchError::Database(format!("sources not encodable: {e}")))?;
        let evidence_json = serde_json::to_string(&evidence)
            .map_err(|e| ResearchError::Database(format!("evidence not encodable: {e}")))?;
        let db = self.db.clone();
        blocking(move || {
            db.put_graph_state(STATE_REPORT, &report_json)?;
            db.put_graph_state(STATE_SOURCES, &sources_json)?;
            db.put_graph_state(STATE_EVIDENCE, &evidence_json)?;
            db.prune_answers().map(|_| ())
        })
        .await?;
        Ok(report)
    }

    /// Matches library papers and their references to OpenAlex works.
    async fn resolve(
        &self,
        scans: Vec<PaperScan>,
        resolver: &Resolver,
        progress: &(dyn Fn(BuildProgress) + Send + Sync),
    ) -> ResearchResult<Vec<ScanInput>> {
        let total: usize = scans.iter().map(|s| 1 + s.identifiable().count()).sum();
        let mut done = 0usize;
        let mut answers: HashMap<WorkRequest, Lookup> = HashMap::new();
        // Library papers first: within a budget or before a rate limit, their identities
        // matter most.
        let mut works: Vec<Option<OpenAlexWork>> = Vec::with_capacity(scans.len());
        let mut scans = scans;
        for scan in &mut scans {
            progress(BuildProgress::Resolving { done, total });
            done += 1;
            works.push(
                self.resolve_library_paper(scan, resolver, &mut answers)
                    .await?,
            );
        }
        let mut inputs = Vec::with_capacity(scans.len());
        for (scan, work) in scans.into_iter().zip(works) {
            let mut reference_works = HashMap::new();
            let references: Vec<_> = scan.identifiable().cloned().collect();
            for reference in references {
                progress(BuildProgress::Resolving { done, total });
                done += 1;
                let p = &reference.parsed;
                let by_identifier = p
                    .doi
                    .clone()
                    .map(WorkRequest::Doi)
                    .or_else(|| p.arxiv_id.as_deref().map(WorkRequest::arxiv));
                let found = match by_identifier {
                    Some(request) => first_work(lookup(resolver, &mut answers, &request).await?)
                        .filter(|w| identifier_match(p.title.as_deref(), w).accepted()),
                    None => {
                        let query = WorkQuery {
                            title: p.title.clone(),
                            year: p.year,
                            first_surname: p.first_surname(),
                        };
                        match (&query.title, query.year, &query.first_surname) {
                            (Some(title), Some(year), Some(_)) => {
                                let request = WorkRequest::Title {
                                    title: title.clone(),
                                    year: Some(year),
                                };
                                match lookup(resolver, &mut answers, &request).await? {
                                    Lookup::Works(works) => best_strict(&query, &works),
                                    _ => None,
                                }
                            }
                            _ => None,
                        }
                    }
                };
                if let Some(w) = found {
                    reference_works.insert(reference.index, w);
                }
            }
            inputs.push(ScanInput {
                scan,
                work,
                reference_works,
                result_methods: Vec::new(),
                result_datasets: Vec::new(),
            });
        }
        progress(BuildProgress::Resolving { done: total, total });
        Ok(inputs)
    }

    /// The OpenAlex work of a library paper, by its DOI or arXiv id; an arXiv id in the
    /// file name counts only when the work has the paper's title (then it is kept).
    async fn resolve_library_paper(
        &self,
        scan: &mut PaperScan,
        resolver: &Resolver,
        answers: &mut HashMap<WorkRequest, Lookup>,
    ) -> ResearchResult<Option<OpenAlexWork>> {
        let identity = scan.identity.clone();
        let request = identity
            .doi
            .clone()
            .map(WorkRequest::Doi)
            .or_else(|| identity.arxiv_id.as_deref().map(WorkRequest::arxiv));
        if let Some(request) = request {
            return Ok(first_work(lookup(resolver, answers, &request).await?)
                .filter(|w| identifier_match(identity.title.as_deref(), w).accepted()));
        }
        if let (Some(hint), Some(title)) = (
            identity.filename_arxiv_id.as_deref(),
            identity.title.as_deref(),
        ) {
            let request = WorkRequest::arxiv(hint);
            if let Some(found) = first_work(lookup(resolver, answers, &request).await?) {
                if titles_agree(title, &found.title) {
                    scan.identity.arxiv_id = Some(hint.to_string());
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }

    async fn previous_sources(&self) -> ResearchResult<BTreeSet<String>> {
        let db = self.db.clone();
        Ok(blocking(move || db.graph_state(STATE_SOURCES))
            .await?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    /// The current graph statements whose source is one of `sources`.
    async fn current(&self, sources: &BTreeSet<String>) -> ResearchResult<Vec<StoredStatement>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for source in sources {
            let rows = self
                .store
                .query(&StatementQuery {
                    classes: GRAPH_CLASSES.iter().map(|c| c.to_string()).collect(),
                    source_prefixes: vec![source.clone()],
                    limit: Some(5_000),
                    ..StatementQuery::default()
                })
                .await?;
            for row in rows {
                let exact = row
                    .statement
                    .provenance
                    .as_ref()
                    .is_some_and(|p| &p.source == source);
                if exact && is_graph_statement(&row) && seen.insert(row.id().to_string()) {
                    out.push(row);
                }
            }
        }
        Ok(out)
    }

    async fn write(
        &self,
        desired: &[DesiredStatement],
        previous_sources: &BTreeSet<String>,
        progress: &(dyn Fn(BuildProgress) + Send + Sync),
    ) -> ResearchResult<Written> {
        let mut sources: BTreeSet<String> = previous_sources.clone();
        sources.extend(desired.iter().map(|d| d.source.clone()));
        let current = self.current(&sources).await?;
        let mut by_key: HashMap<(String, String), StoredStatement> = HashMap::new();
        let mut duplicates: Vec<String> = Vec::new();
        for row in current {
            let key = (
                row.statement.class.clone(),
                row.statement
                    .subject
                    .as_ref()
                    .map(|s| s.id.clone())
                    .unwrap_or_default(),
            );
            match by_key.get(&key) {
                Some(existing) if existing.valid_from >= row.valid_from => {
                    duplicates.push(row.id().to_string())
                }
                Some(_) => {
                    if let Some(old) = by_key.insert(key, row) {
                        duplicates.push(old.id().to_string());
                    }
                }
                None => {
                    by_key.insert(key, row);
                }
            }
        }
        let now = self.store.now();
        let mut written = Written::default();
        let wanted: HashSet<(String, String)> = desired
            .iter()
            .map(|d| (d.class.to_string(), d.subject.clone()))
            .collect();
        for (done, d) in desired.iter().enumerate() {
            if done % 25 == 0 {
                progress(BuildProgress::Writing {
                    done,
                    total: desired.len(),
                });
            }
            let key = (d.class.to_string(), d.subject.clone());
            let existing = by_key.get(&key);
            if let Some(e) = existing {
                let same_source = e
                    .statement
                    .provenance
                    .as_ref()
                    .is_some_and(|p| p.source == d.source);
                if e.statement.properties == d.properties && same_source {
                    written.unchanged += 1;
                    written
                        .ids
                        .insert((d.class, d.subject.clone()), e.id().to_string());
                    continue;
                }
            }
            let statement = Statement {
                id: format!("graph-{}", uuid::Uuid::new_v4()),
                class: d.class.to_string(),
                subject: Some(shodh_ontology::EntityRef::typed(d.subject.clone(), d.class)),
                properties: d.properties.clone(),
                ontology_version: self.version_of(d.class),
                valid_from: Some(now),
                provenance: Some(d.provenance(now)),
            };
            let intent = match existing {
                Some(e) => PutIntent::Supersede {
                    target: e.id().to_string(),
                },
                None => PutIntent::Auto,
            };
            match self.store.put(statement, Scope::Global, intent).await {
                Ok(PutOutcome::Added { id }) => {
                    written.added += 1;
                    written.ids.insert((d.class, d.subject.clone()), id);
                }
                Ok(PutOutcome::Updated { id, .. }) => {
                    written.updated += 1;
                    written.ids.insert((d.class, d.subject.clone()), id);
                }
                Ok(PutOutcome::Unchanged { existing })
                | Ok(PutOutcome::Historical {
                    current: existing, ..
                }) => {
                    written.unchanged += 1;
                    written.ids.insert((d.class, d.subject.clone()), existing);
                }
                Ok(PutOutcome::Conflict { existing, .. }) => {
                    tracing::warn!(target: "shodh::research", subject = %d.subject, %existing, "graph statement conflicts with a statement made elsewhere; left as it is");
                    written.refused += 1;
                }
                Err(StatementError::Invalid(violations)) => {
                    tracing::warn!(target: "shodh::research", subject = %d.subject, violations = violations.len(), "graph statement refused by the ontology");
                    written.refused += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
        for (key, row) in &by_key {
            if !wanted.contains(key) {
                self.store.forget(row.id()).await?;
                written.removed += 1;
            }
        }
        for id in duplicates {
            self.store.forget(&id).await?;
            written.removed += 1;
        }
        progress(BuildProgress::Writing {
            done: desired.len(),
            total: desired.len(),
        });
        Ok(written)
    }

    /// Reads the graph from the statement store (and the citation evidence of the last
    /// build). Methods and datasets are named from the accepted Result statements.
    async fn load(&self, results: &ResultService) -> ResearchResult<PaperGraph> {
        let sources = self.previous_sources().await?;
        if sources.is_empty() {
            return Ok(PaperGraph::default());
        }
        let rows = self.current(&sources).await?;
        let mut parts = GraphParts::default();
        let mut venue_names: HashMap<String, String> = HashMap::new();
        for row in rows.iter().filter(|r| r.statement.class == "Venue") {
            if let (Some(subject), Some(name)) =
                (&row.statement.subject, text_value(&row.statement, "name"))
            {
                venue_names.insert(subject.id.clone(), name);
            }
        }
        let mut uses = Vec::new();
        let mut evaluated = Vec::new();
        for row in &rows {
            let s = &row.statement;
            let Some(subject) = s.subject.as_ref().map(|e| e.id.clone()) else {
                continue;
            };
            match s.class.as_str() {
                "Paper" => {
                    let in_library =
                        matches!(s.properties.get("inLibrary"), Some(RawValue::Boolean(true)));
                    let source = s.provenance.as_ref().map(|p| p.source.clone());
                    for cited in entity_values(s, "cites") {
                        parts.cites.push((subject.clone(), cited, None));
                    }
                    uses.extend(
                        entity_values(s, "usesMethod")
                            .into_iter()
                            .map(|m| (subject.clone(), m)),
                    );
                    evaluated.extend(
                        entity_values(s, "evaluatedOn")
                            .into_iter()
                            .map(|d| (subject.clone(), d)),
                    );
                    parts.papers.push(PaperNode {
                        id: subject.clone(),
                        title: text_value(s, "title"),
                        year: integer_value(s, "publicationYear")
                            .and_then(|y| i32::try_from(y).ok()),
                        doi: text_value(s, "doi"),
                        arxiv_id: text_value(s, "arxivId")
                            .or_else(|| subject.strip_prefix("paper:arxiv:").map(str::to_string)),
                        openalex_id: text_value(s, "openalexId"),
                        authors: text_value(s, "authorsAsPrinted"),
                        author_ids: entity_values(s, "authoredBy"),
                        venue: entity_values(s, "publishedIn")
                            .first()
                            .and_then(|v| venue_names.get(v).cloned()),
                        in_library,
                        file_path: if in_library { source } else { None },
                        cited_by_count: integer_value(s, "citedByCount"),
                        statement_id: Some(row.id().to_string()),
                    });
                }
                "Author" => {
                    if let Some(name) = text_value(s, "name") {
                        parts.authors.insert(subject, name);
                    }
                }
                "Method" => {
                    if let Some(paper) = entity_values(s, "proposedIn").first() {
                        parts.proposed_in.push((subject, paper.clone()));
                    }
                }
                _ => {}
            }
        }
        let facets = results.facets(&[]).await?;
        let mut methods: BTreeMap<String, String> = facets
            .methods
            .into_iter()
            .map(|f| (f.id, f.label))
            .collect();
        for row in rows.iter().filter(|r| r.statement.class == "Method") {
            if let (Some(subject), Some(name)) =
                (&row.statement.subject, text_value(&row.statement, "name"))
            {
                methods.entry(subject.id.clone()).or_insert(name);
            }
        }
        parts.methods = methods
            .into_iter()
            .map(|(id, label)| ConceptNode { id, label })
            .collect();
        parts.datasets = facets
            .datasets
            .into_iter()
            .map(|f| ConceptNode {
                id: f.id,
                label: f.label,
            })
            .collect();
        parts.uses_method = uses;
        parts.evaluated_on = evaluated;
        let db = self.db.clone();
        let evidence: Vec<(String, String, CitationEvidence)> =
            blocking(move || db.graph_state(STATE_EVIDENCE))
                .await?
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();
        let evidence: HashMap<(String, String), CitationEvidence> =
            evidence.into_iter().map(|(a, b, e)| ((a, b), e)).collect();
        for (a, b, e) in &mut parts.cites {
            *e = evidence.get(&(a.clone(), b.clone())).cloned();
        }
        Ok(PaperGraph::new(parts))
    }
}

/// What the scanning step of a build did.
#[derive(Debug, Default)]
pub(crate) struct ScanCounts {
    pub files: usize,
    pub parsed: usize,
    pub reused: usize,
    pub failed: Vec<FailedFile>,
}

#[derive(Debug, Default)]
struct Written {
    added: usize,
    updated: usize,
    unchanged: usize,
    removed: usize,
    refused: usize,
    ids: HashMap<(&'static str, String), String>,
}

fn reject_label(reason: RejectReason) -> &'static str {
    match reason {
        RejectReason::OutsideBibliography => "outside the bibliography",
        RejectReason::TooLong => "too long for a reference",
        RejectReason::TooShort => "too short",
        RejectReason::NotAReference => "not shaped like a reference",
        RejectReason::Orphan => "fragment without an entry",
    }
}

fn first_work(lookup: Lookup) -> Option<OpenAlexWork> {
    match lookup {
        Lookup::Works(mut works) if !works.is_empty() => Some(works.remove(0)),
        _ => None,
    }
}

fn best_strict(query: &WorkQuery, works: &[OpenAlexWork]) -> Option<OpenAlexWork> {
    works
        .iter()
        .filter_map(|w| match strict_match(query, w) {
            MatchVerdict::Accepted { similarity } => Some((similarity, w)),
            MatchVerdict::Rejected { .. } => None,
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, w)| w.clone())
}

/// One lookup per distinct request in a build.
async fn lookup(
    resolver: &Resolver,
    answers: &mut HashMap<WorkRequest, Lookup>,
    request: &WorkRequest,
) -> ResearchResult<Lookup> {
    if let Some(answer) = answers.get(request) {
        return Ok(answer.clone());
    }
    let answer = resolver.lookup(request).await?;
    answers.insert(request.clone(), answer.clone());
    Ok(answer)
}
