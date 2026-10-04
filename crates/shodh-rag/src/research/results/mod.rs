//! Result statements read from the structured table blocks of papers, their review, and
//! the cross-paper comparison built from them.
//!
//! Extraction ([`ResultService::extract`]) parses the PDF, reads every table block with the
//! header-aware rules of [`interpret`] (asking a language model only to name the dataset or
//! metric of columns the rules could not, when the caller supplies one), validates each
//! value as a research-pack `Result` statement and stores it with its provenance:
//! extractor kind (`rule`, `llm`), confidence, page and the cell's box. Values below
//! [`ACCEPT_THRESHOLD`] wait in a review list and are never used until the user accepts
//! them; rejected values are remembered and not extracted again.
//!
//! Every value occurs verbatim in its cell ([`numbers`]); nothing is computed, rounded or
//! converted, and the model never supplies a value.

pub mod interpret;
pub mod numbers;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use shodh_ontology::{EntityRef, Extractor, ExtractorKind, Provenance, RawValue, Statement};

use self::interpret::{find_metric, read_table, verify_roles, Candidate, HeaderRoles, TableInput};
use super::db::ResearchDb;
use super::pdf_text::format_points;
use super::{
    blocking, canonical_file, document_entity, file_name, path_key, ResearchError, ResearchResult,
};
use crate::processing::document_model::{is_table_caption, BBox, BlockKind, StructuredDocument};
use crate::processing::pdf_layout::{parse_pdf_layout_with, TableMode};
use crate::processing::table_model::{loaded, SharedTableModel, TABLE_MODEL_ID};
use crate::statements::{
    PropertyFilter, PutIntent, PutOutcome, Scope, StatementError, StatementQuery, StatementStore,
    StoredStatement,
};

/// Class of result statements.
pub const RESULT_CLASS: &str = "Result";
/// Values at or above this confidence are used without review.
pub const ACCEPT_THRESHOLD: f64 = 0.8;
/// Version of the deterministic rules, recorded as the extractor version.
pub const RULES_VERSION: &str = "results-rules/1";
/// Most result statements read for one listing or comparison.
const MAX_READ: usize = 5_000;
/// Most method rows in one comparison.
pub const MAX_COMPARISON_ROWS: usize = 200;
/// Most papers named in one coverage note.
const NAMED_PAPERS: usize = 5;

/// Canonical entity id of a method, dataset or metric label: `kind:` and the label lower-
/// cased with everything but letters, digits, `@` and `.` between digits removed. Metrics
/// use the lexicon's canonical name when the label names a known measure (`R@10` →
/// `metric:recall@10`).
pub fn canonical_id(kind: &str, label: &str) -> String {
    let base = if kind == "metric" {
        find_metric(label)
            .map(|(name, _)| name)
            .unwrap_or_else(|| label.to_string())
    } else {
        label.to_string()
    };
    let chars: Vec<char> = base.to_lowercase().chars().collect();
    let mut slug = String::new();
    for (i, c) in chars.iter().enumerate() {
        let keep_dot = *c == '.'
            && i > 0
            && chars[i - 1].is_ascii_digit()
            && chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if c.is_alphanumeric() || *c == '@' || keep_dot {
            slug.push(*c);
        } else if *c == '-' && kind == "metric" && !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    format!("{kind}:{}", slug.trim_end_matches('-'))
}

/// One Result statement as the app shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRecord {
    pub id: String,
    pub method: String,
    pub dataset: String,
    pub metric: String,
    pub method_id: String,
    pub dataset_id: String,
    pub metric_id: String,
    pub value: String,
    pub value_text: String,
    pub unit: Option<String>,
    /// The `±` spread printed after the value.
    pub spread: Option<String>,
    pub setting: Option<String>,
    pub file_path: String,
    pub file_name: String,
    pub page: u32,
    pub region: Option<Region>,
    pub table_caption: Option<String>,
    pub extractor: ExtractorKind,
    pub confidence: f64,
    pub status: ResultStatus,
    #[serde(skip)]
    pub scope: Scope,
    #[serde(skip)]
    pub extracted_at: DateTime<Utc>,
}

/// Whether a result is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultStatus {
    Accepted,
    Review,
}

/// A box on a page, bottom-left origin (as citation `regions`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub page: u32,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// A table that produced no results, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedTable {
    pub page: u32,
    pub caption: Option<String>,
    pub reason: String,
}

/// What one extraction did; the last one per paper is kept in `shodh.db`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionReport {
    pub file_path: String,
    pub file_name: String,
    pub tables: usize,
    pub added: usize,
    pub unchanged: usize,
    pub review: usize,
    pub conflicts: usize,
    pub skipped: Vec<SkippedTable>,
    pub model: Option<String>,
    /// The table model that structured the paper's tables; `None` when they came from
    /// the layout heuristics alone (reduced table quality).
    #[serde(default)]
    pub table_model: Option<String>,
    pub extracted_at: DateTime<Utc>,
}

/// The results of one paper.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperResults {
    pub results: Vec<ResultRecord>,
    pub review: Vec<ResultRecord>,
    pub report: Option<ExtractionReport>,
}

/// Filters of a comparison. Labels or canonical ids, compared by canonical id.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultFilter {
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub dataset: Option<String>,
    #[serde(default)]
    pub metric: Option<String>,
    /// Only these papers (file paths).
    #[serde(default)]
    pub papers: Vec<String>,
    /// Only these scopes; empty means all.
    #[serde(skip)]
    pub scopes: Vec<Scope>,
    /// The PDFs in scope that the index knows (for "not yet scanned" notes).
    #[serde(skip)]
    pub known_papers: Vec<String>,
}

/// One value of a comparison cell.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonCell {
    pub result_id: String,
    pub value: String,
    pub value_text: String,
    pub unit: Option<String>,
    pub spread: Option<String>,
    pub file_path: String,
    pub file_name: String,
    pub page: u32,
    pub region: Option<Region>,
    pub setting: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonRow {
    pub method: String,
    pub method_id: String,
    pub cells: BTreeMap<String, Vec<ComparisonCell>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonColumn {
    pub key: String,
    pub dataset: String,
    pub metric: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperRef {
    pub file_path: String,
    pub file_name: String,
}

/// A method × (dataset, metric) table of accepted results, with coverage notes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub columns: Vec<ComparisonColumn>,
    pub rows: Vec<ComparisonRow>,
    pub papers: Vec<PaperRef>,
    pub notes: Vec<String>,
    pub pending_review: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetValue {
    pub id: String,
    pub label: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetPaper {
    pub file_path: String,
    pub file_name: String,
    pub results: usize,
}

/// Values to choose from when building a comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultFacets {
    pub methods: Vec<FacetValue>,
    pub datasets: Vec<FacetValue>,
    pub metrics: Vec<FacetValue>,
    pub papers: Vec<FacetPaper>,
}

/// A language model that names the dataset and metric of table columns. Implementations
/// send `prompt` and return the reply text; the service validates what comes back.
#[async_trait::async_trait]
pub trait TextModel: Send + Sync {
    /// Model id recorded as the extractor version.
    fn model_id(&self) -> String;
    /// Completes `prompt` with at most `max_tokens` output tokens.
    async fn complete(&self, prompt: &str, max_tokens: usize) -> Result<String, String>;
}

/// Longest model reply read.
const MODEL_OUTPUT_TOKENS: usize = 600;

/// The prompt asking a model to name the dataset, metric and setting of `columns`. It
/// carries the ontology's definitions of the three roles and says that labels must be
/// copied from the table text; values are not asked for.
pub fn roles_prompt(table: &TableInput, columns: &[usize], definitions: &str) -> String {
    let mut grid = String::new();
    let width = std::iter::once(table.header.len())
        .chain(table.rows.iter().map(Vec::len))
        .max()
        .unwrap_or(0);
    let line = |cells: &[String]| {
        (0..width)
            .map(|i| {
                cells
                    .get(i)
                    .map(String::as_str)
                    .unwrap_or("")
                    .replace('|', "/")
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };
    grid.push_str(&format!("header: {}\n", line(&table.header)));
    for (i, row) in table.rows.iter().take(6).enumerate() {
        grid.push_str(&format!("row {i}: {}\n", line(row)));
    }
    format!(
        "You label the columns of a results table from a research paper. Use only these \
definitions:\n{definitions}\n\nTable (columns are numbered from 0, cells separated by |):\n{grid}\
Caption: {caption}\nNearby caption: {nearby}\nSection: {section}\n\n\
For each of the columns {columns:?}, name the dataset (benchmark) and the metric its numbers \
report, and a setting if the header gives one. Copy every name exactly as it is written in the \
table, caption or section above; if a name is not written there, use null. Never write numbers \
from the cells. Reply with JSON only: {{\"columns\": [{{\"index\": 1, \"metric\": \"...\", \
\"dataset\": \"...\", \"setting\": null}}]}}",
        caption = table.caption.as_deref().unwrap_or("(none)"),
        nearby = table.nearby_caption.as_deref().unwrap_or("(none)"),
        section = if table.section_path.is_empty() {
            "(none)".to_string()
        } else {
            table.section_path.join(" > ")
        },
    )
}

/// The JSON object in a model reply (it may be wrapped in prose or a code fence).
pub fn parse_roles(reply: &str) -> Option<HeaderRoles> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&reply[start..=end]).ok()
}

/// The table blocks of a parsed document as interpreter input. A table the parser left
/// without a caption gets the nearest `Table N` caption paragraph on its page as weaker
/// evidence.
pub fn tables_of(doc: &StructuredDocument) -> Vec<TableInput> {
    let captions: Vec<(u32, BBox, &str)> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Paragraph) && is_table_caption(&b.text))
        .filter_map(|b| Some((b.page?, b.bbox?, b.text.as_str())))
        .collect();
    let mut out = Vec::new();
    for block in &doc.blocks {
        let BlockKind::Table {
            header,
            rows,
            caption,
            cell_boxes,
        } = &block.kind
        else {
            continue;
        };
        let Some(page) = block.page else { continue };
        let nearby = match (caption, block.bbox) {
            (None, Some(bbox)) => captions
                .iter()
                .filter(|(p, _, _)| *p == page)
                .min_by(|a, b| vertical_gap(&a.1, &bbox).total_cmp(&vertical_gap(&b.1, &bbox)))
                .map(|(_, _, text)| text.to_string()),
            _ => None,
        };
        out.push(TableInput {
            page,
            bbox: block.bbox,
            caption: caption.clone(),
            nearby_caption: nearby,
            section_path: block.section_path.clone(),
            header: header.clone(),
            rows: rows.clone(),
            cell_boxes: cell_boxes.clone(),
        });
    }
    out
}

/// Candidates that are not stored: the same method, dataset, metric and setting twice
/// in one paper with different values is ambiguous, so neither is kept; with the same
/// value, only the first is.
pub fn ambiguous<'a>(candidates: impl Iterator<Item = &'a Candidate>) -> HashSet<usize> {
    let mut by_identity: HashMap<(String, String, String, String), Vec<usize>> = HashMap::new();
    let mut values: Vec<&str> = Vec::new();
    for (i, c) in candidates.enumerate() {
        values.push(c.number.decimal.as_str());
        by_identity
            .entry((
                canonical_id("method", &c.method),
                canonical_id("dataset", &c.dataset),
                canonical_id("metric", &c.metric),
                c.setting.clone(),
            ))
            .or_default()
            .push(i);
    }
    let mut drop: HashSet<usize> = HashSet::new();
    for indices in by_identity.values().filter(|v| v.len() > 1) {
        let distinct: BTreeSet<&str> = indices.iter().map(|i| values[*i]).collect();
        if distinct.len() > 1 {
            drop.extend(indices.iter().copied());
        } else {
            drop.extend(indices.iter().skip(1).copied());
        }
    }
    drop
}

fn vertical_gap(a: &BBox, b: &BBox) -> f32 {
    if a.y1 < b.y0 {
        b.y0 - a.y1
    } else if b.y1 < a.y0 {
        a.y0 - b.y1
    } else {
        0.0
    }
}

fn text_of(properties: &BTreeMap<String, RawValue>, key: &str) -> Option<String> {
    match properties.get(key)? {
        RawValue::Text(t) => Some(t.clone()),
        RawValue::Integer(i) => Some(i.to_string()),
        RawValue::Float(f) => Some(f.to_string()),
        _ => None,
    }
}

fn entity_of(properties: &BTreeMap<String, RawValue>, key: &str) -> Option<String> {
    match properties.get(key)? {
        RawValue::Entity(e) => Some(e.id.clone()),
        _ => None,
    }
}

fn region_text(region: &Region) -> String {
    [region.x0, region.y0, region.x1, region.y1]
        .iter()
        .map(|v| format_points(v.max(0.0)))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_region(page: u32, text: &str) -> Option<Region> {
    let v: Vec<f32> = text
        .split(',')
        .map(|p| p.trim().parse::<f32>().ok())
        .collect::<Option<Vec<_>>>()?;
    match v.as_slice() {
        [x0, y0, x1, y1] => Some(Region {
            page,
            x0: *x0,
            y0: *y0,
            x1: *x1,
            y1: *y1,
        }),
        _ => None,
    }
}

/// Identity of a value in one paper for rejections: file, page, cell box, cell text and
/// the three roles.
fn fingerprint(
    path: &str,
    page: u32,
    region: Option<&str>,
    cell: &str,
    roles: [&str; 3],
) -> String {
    let material = [
        path,
        &page.to_string(),
        region.unwrap_or(""),
        cell,
        roles[0],
        roles[1],
        roles[2],
    ]
    .join("\u{1f}");
    hex::encode(Sha256::digest(material.as_bytes()))
}

/// Decodes a stored `Result` statement.
pub fn record_from(stored: &StoredStatement) -> ResearchResult<ResultRecord> {
    let s = &stored.statement;
    let corrupt =
        |what: &str| ResearchError::Database(format!("result statement `{}` has no {what}", s.id));
    let provenance = s.provenance.as_ref().ok_or_else(|| corrupt("provenance"))?;
    let p = &s.properties;
    let method_id = entity_of(p, "resultMethod").ok_or_else(|| corrupt("method"))?;
    let dataset_id = entity_of(p, "resultDataset").ok_or_else(|| corrupt("dataset"))?;
    let metric_id = entity_of(p, "resultMetric").ok_or_else(|| corrupt("metric"))?;
    let value = text_of(p, "resultValue").ok_or_else(|| corrupt("value"))?;
    let page = text_of(p, "resultPage")
        .and_then(|t| t.parse::<u32>().ok())
        .or(provenance.page)
        .unwrap_or(0);
    let label = |key: &str, id: &str| {
        text_of(p, key).unwrap_or_else(|| id.split_once(':').map(|x| x.1).unwrap_or(id).to_string())
    };
    let confidence = provenance.confidence;
    let status =
        if provenance.extractor.kind == ExtractorKind::User || confidence >= ACCEPT_THRESHOLD {
            ResultStatus::Accepted
        } else {
            ResultStatus::Review
        };
    Ok(ResultRecord {
        id: s.id.clone(),
        method: label("resultMethodLabel", &method_id),
        dataset: label("resultDatasetLabel", &dataset_id),
        metric: label("resultMetricLabel", &metric_id),
        method_id,
        dataset_id,
        metric_id,
        value_text: text_of(p, "resultValueText").unwrap_or_else(|| value.clone()),
        value,
        unit: text_of(p, "resultUnit"),
        spread: text_of(p, "resultSpread"),
        setting: text_of(p, "resultSetting"),
        file_path: provenance.source.clone(),
        file_name: file_name(&provenance.source),
        page,
        region: text_of(p, "resultRegion").and_then(|r| parse_region(page, &r)),
        table_caption: text_of(p, "resultTableCaption"),
        extractor: provenance.extractor.kind,
        confidence,
        status,
        scope: stored.scope.clone(),
        extracted_at: provenance.extracted_at,
    })
}

fn record_fingerprint(record: &ResultRecord) -> String {
    let region = record.region.as_ref().map(region_text);
    fingerprint(
        &path_key(&record.file_path),
        record.page,
        region.as_deref(),
        &record.value_text,
        [&record.method_id, &record.dataset_id, &record.metric_id],
    )
}

/// Results, their review and comparisons over the statement store and `shodh.db`.
pub struct ResultService {
    store: Arc<StatementStore>,
    db: Arc<ResearchDb>,
    app_version: String,
    /// Structures the tables of candidate pages when installed.
    tables: SharedTableModel,
}

impl std::fmt::Debug for ResultService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultService").finish_non_exhaustive()
    }
}

/// A candidate ready to store.
struct Prepared {
    statement: Statement,
    review: bool,
}

impl ResultService {
    pub fn new(store: Arc<StatementStore>, db: Arc<ResearchDb>, app_version: &str) -> Self {
        Self {
            store,
            db,
            app_version: app_version.to_string(),
            tables: SharedTableModel::default(),
        }
    }

    /// Parses papers with the table model in `tables` whenever one is loaded.
    pub fn with_table_model(mut self, tables: SharedTableModel) -> Self {
        self.tables = tables;
        self
    }

    fn version(&self) -> semver::Version {
        let ontology = self.store.ontology();
        ontology
            .class(RESULT_CLASS)
            .and_then(|c| ontology.source(&c.source))
            .map(|s| s.version.clone())
            .unwrap_or_else(|| ontology.version().clone())
    }

    /// The ontology's definitions of the roles a model may name (the slice it is
    /// constrained by).
    pub fn role_definitions(&self) -> String {
        let ontology = self.store.ontology();
        ["Result", "Method", "Dataset", "Metric"]
            .iter()
            .filter_map(|id| ontology.class(id))
            .map(|c| format!("- {}: {}", c.label, c.description))
            .chain(
                ["resultSetting"]
                    .iter()
                    .filter_map(|id| ontology.property(id))
                    .map(|p| format!("- setting: {}", p.description)),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Parses the PDF at `path` and extracts its results (see the module documentation).
    pub async fn extract(
        &self,
        path: &str,
        scope: Scope,
        model: Option<Arc<dyn TextModel>>,
    ) -> ResearchResult<ExtractionReport> {
        let path = canonical_file(path);
        let read_path = path.clone();
        let table_model = loaded(&self.tables);
        let used = table_model.as_ref().map(|_| TABLE_MODEL_ID.to_string());
        let doc = blocking(move || {
            let bytes = std::fs::read(&read_path).map_err(|e| {
                ResearchError::Pdf(format!("{} could not be read: {e}", file_name(&read_path)))
            })?;
            let mode = match &table_model {
                Some(m) => TableMode::Model(m),
                None => TableMode::Heuristic,
            };
            parse_pdf_layout_with(&bytes, mode)
                .map(|parsed| parsed.document)
                .map_err(|e| ResearchError::Pdf(format!("The PDF could not be parsed: {e}")))
        })
        .await?;
        self.extract_parsed(&path, &doc, used, scope, model).await
    }

    /// Re-extracts a paper whose tables the table model has just structured, when it
    /// was scanned for results before (a paper never scanned stays unscanned). The
    /// values keep the scope of the paper's current results.
    pub async fn reextract_if_scanned(
        &self,
        path: &str,
        doc: &StructuredDocument,
    ) -> ResearchResult<Option<ExtractionReport>> {
        let path = canonical_file(path);
        let key = path_key(&path);
        let db = self.db.clone();
        if blocking(move || db.report(&key)).await?.is_none() {
            return Ok(None);
        }
        let scope = self
            .current_of(&path)
            .await?
            .first()
            .map(|r| r.scope.clone())
            .unwrap_or_else(|| Scope::for_workspace(None));
        self.extract_parsed(&path, doc, Some(TABLE_MODEL_ID.to_string()), scope, None)
            .await
            .map(Some)
    }

    /// Extracts the results of an already parsed document (tables from the heuristics).
    pub async fn extract_document(
        &self,
        path: &str,
        doc: &StructuredDocument,
        scope: Scope,
        model: Option<Arc<dyn TextModel>>,
    ) -> ResearchResult<ExtractionReport> {
        self.extract_parsed(path, doc, None, scope, model).await
    }

    /// Extracts the results of a parsed document whose tables `table_model` structured
    /// (`None`: the heuristics).
    async fn extract_parsed(
        &self,
        path: &str,
        doc: &StructuredDocument,
        table_model: Option<String>,
        scope: Scope,
        model: Option<Arc<dyn TextModel>>,
    ) -> ResearchResult<ExtractionReport> {
        let path = canonical_file(path);
        let tables = tables_of(doc);
        let definitions = self.role_definitions();
        let mut skipped = Vec::new();
        let mut candidates: Vec<(Candidate, Option<String>, ExtractorKind, String)> = Vec::new();
        let mut used_model: Option<String> = None;
        for table in &tables {
            let caption = table
                .caption
                .clone()
                .or_else(|| table.nearby_caption.clone());
            let mut reading = read_table(table, None);
            let mut extractor = (ExtractorKind::Rule, RULES_VERSION.to_string());
            if !reading.unresolved.is_empty() {
                if let Some(model) = &model {
                    let width = std::iter::once(table.header.len())
                        .chain(table.rows.iter().map(Vec::len))
                        .max()
                        .unwrap_or(0);
                    let prompt = roles_prompt(table, &reading.unresolved, &definitions);
                    match model.complete(&prompt, MODEL_OUTPUT_TOKENS).await {
                        Ok(reply) => match parse_roles(&reply) {
                            Some(roles) => {
                                let (verified, refused) = verify_roles(table, roles, width);
                                if refused > 0 {
                                    tracing::info!(target: "shodh::research", refused, page = table.page, "model labels not in the table text were refused");
                                }
                                let with_model = read_table(table, Some(&verified));
                                if with_model.candidates.len() > reading.candidates.len() {
                                    reading = with_model;
                                    extractor = (ExtractorKind::Llm, model.model_id());
                                }
                                used_model = Some(model.model_id());
                            }
                            None => {
                                tracing::info!(target: "shodh::research", page = table.page, "model reply was not the expected JSON")
                            }
                        },
                        Err(e) => {
                            return Err(ResearchError::Model(format!(
                                "The model could not be asked about the table on page {}: {e}",
                                table.page
                            )))
                        }
                    }
                }
            }
            if reading.candidates.is_empty() {
                skipped.push(SkippedTable {
                    page: table.page,
                    caption: caption.clone(),
                    reason: reading
                        .skipped
                        .clone()
                        .unwrap_or_else(|| "no values could be read".to_string()),
                });
                continue;
            }
            for candidate in reading.candidates {
                // Model-named roles carry the model's extractor; rule-named ones keep the rules.
                let kind = if candidate.metric_evidence == interpret::Evidence::Model
                    || candidate.dataset_evidence == interpret::Evidence::Model
                {
                    extractor.0
                } else {
                    ExtractorKind::Rule
                };
                let version = if kind == ExtractorKind::Rule {
                    RULES_VERSION.to_string()
                } else {
                    extractor.1.clone()
                };
                candidates.push((candidate, caption.clone(), kind, version));
            }
        }

        let drop = ambiguous(candidates.iter().map(|(c, _, _, _)| c));
        if !drop.is_empty() {
            tracing::info!(target: "shodh::research", dropped = drop.len(), "ambiguous duplicate values not stored");
        }

        let key = path_key(&path);
        let rejected = {
            let db = self.db.clone();
            let k = key.clone();
            blocking(move || db.rejected(&k)).await?
        };
        let now = self.store.now();
        let mut prepared: Vec<Prepared> = Vec::new();
        for (i, (c, caption, kind, version)) in candidates.into_iter().enumerate() {
            if drop.contains(&i) {
                continue;
            }
            let method_id = canonical_id("method", &c.method);
            let dataset_id = canonical_id("dataset", &c.dataset);
            let metric_id = canonical_id("metric", &c.metric);
            let region = c.cell_box.map(|b| Region {
                page: c.page,
                x0: b.x0,
                y0: b.y0,
                x1: b.x1,
                y1: b.y1,
            });
            let region_text = region.as_ref().map(region_text);
            let fp = fingerprint(
                &key,
                c.page,
                region_text.as_deref(),
                &c.cell_text,
                [&method_id, &dataset_id, &metric_id],
            );
            if rejected.contains(&fp) {
                continue;
            }
            let mut properties: BTreeMap<String, RawValue> = BTreeMap::new();
            properties.insert(
                "resultMethod".into(),
                RawValue::Entity(EntityRef::typed(method_id, "Method")),
            );
            properties.insert(
                "resultDataset".into(),
                RawValue::Entity(EntityRef::typed(dataset_id, "Dataset")),
            );
            properties.insert(
                "resultMetric".into(),
                RawValue::Entity(EntityRef::typed(metric_id, "Metric")),
            );
            properties.insert(
                "resultValue".into(),
                RawValue::text(c.number.decimal.clone()),
            );
            properties.insert(
                "resultValueText".into(),
                RawValue::text(c.cell_text.clone()),
            );
            if let Some(unit) = &c.unit {
                properties.insert("resultUnit".into(), RawValue::text(unit.clone()));
            }
            if let Some(spread) = &c.number.spread {
                properties.insert("resultSpread".into(), RawValue::text(spread.clone()));
            }
            if !c.setting.is_empty() {
                properties.insert("resultSetting".into(), RawValue::text(c.setting.clone()));
            }
            let mut paper = document_entity(&path);
            paper.class = Some("Paper".to_string());
            properties.insert("reportedIn".into(), RawValue::Entity(paper));
            properties.insert("resultMethodLabel".into(), RawValue::text(c.method.clone()));
            properties.insert(
                "resultDatasetLabel".into(),
                RawValue::text(c.dataset.clone()),
            );
            properties.insert("resultMetricLabel".into(), RawValue::text(c.metric.clone()));
            if let Some(caption) = &caption {
                properties.insert(
                    "resultTableCaption".into(),
                    RawValue::text(caption.chars().take(500).collect::<String>()),
                );
            }
            properties.insert("resultPage".into(), RawValue::Integer(i64::from(c.page)));
            if let Some(text) = region_text {
                properties.insert("resultRegion".into(), RawValue::text(text));
            }
            prepared.push(Prepared {
                review: c.confidence < ACCEPT_THRESHOLD,
                statement: Statement {
                    id: format!("res-{}", uuid::Uuid::new_v4()),
                    class: RESULT_CLASS.to_string(),
                    subject: None,
                    properties,
                    ontology_version: self.version(),
                    valid_from: Some(now),
                    provenance: Some(Provenance {
                        source: path.clone(),
                        generation: 0,
                        page: Some(c.page),
                        span: None,
                        extractor: Extractor { kind, version },
                        confidence: c.confidence,
                        extracted_at: now,
                    }),
                },
            });
        }

        let existing = self.current_of(&path).await?;
        let mut kept: HashSet<String> = HashSet::new();
        let (mut added, mut unchanged, mut review, mut conflicts) = (0, 0, 0, 0);
        for item in prepared {
            let outcome = self
                .store
                .put(item.statement.clone(), scope.clone(), PutIntent::Auto)
                .await;
            let outcome = match outcome {
                Ok(o) => o,
                Err(StatementError::Invalid(violations)) => {
                    tracing::warn!(target: "shodh::research", ?violations, "result statement rejected by the ontology");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            match outcome {
                PutOutcome::Added { id } | PutOutcome::Updated { id, .. } => {
                    added += 1;
                    if item.review {
                        review += 1;
                    }
                    kept.insert(id);
                }
                PutOutcome::Unchanged { existing }
                | PutOutcome::Historical {
                    current: existing, ..
                } => {
                    unchanged += 1;
                    kept.insert(existing);
                }
                PutOutcome::Conflict { existing, .. } => {
                    let by_user = existing_by_user(&self.store, &existing).await?;
                    if by_user {
                        conflicts += 1;
                        kept.insert(existing);
                        continue;
                    }
                    let outcome = self
                        .store
                        .put(
                            item.statement,
                            scope.clone(),
                            PutIntent::Supersede {
                                target: existing.clone(),
                            },
                        )
                        .await?;
                    if let Some(id) = outcome.stored_id() {
                        added += 1;
                        if item.review {
                            review += 1;
                        }
                        kept.insert(id.to_string());
                    }
                }
            }
        }
        // Values an earlier extraction stored that this one no longer reads are removed,
        // unless the user accepted them.
        for old in existing {
            if kept.contains(&old.id) || old.extractor == ExtractorKind::User {
                continue;
            }
            match self.store.forget(&old.id).await {
                Ok(_) | Err(StatementError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let report = ExtractionReport {
            file_name: file_name(&path),
            file_path: path.clone(),
            tables: tables.len(),
            added,
            unchanged,
            review,
            conflicts,
            skipped,
            model: used_model,
            table_model,
            extracted_at: now,
        };
        let json = serde_json::to_string(&report)
            .map_err(|e| ResearchError::Database(format!("report could not be encoded: {e}")))?;
        let db = self.db.clone();
        blocking(move || db.put_report(&key, &json)).await?;
        Ok(report)
    }

    /// Current result statements reported in the paper at `path` (any spelling of it).
    async fn current_of(&self, path: &str) -> ResearchResult<Vec<ResultRecord>> {
        let query = StatementQuery {
            classes: vec![RESULT_CLASS.to_string()],
            properties: vec![PropertyFilter {
                property: "reportedIn".to_string(),
                equals: format!("@{}", document_entity(path).id),
            }],
            limit: Some(MAX_READ),
            ..StatementQuery::default()
        };
        let mut out = Vec::new();
        for row in self.store.query(&query).await? {
            match record_from(&row) {
                Ok(r) => out.push(r),
                Err(e) => {
                    tracing::warn!(target: "shodh::research", error = %e, "unreadable result skipped")
                }
            }
        }
        Ok(out)
    }

    async fn all(&self, scopes: &[Scope]) -> ResearchResult<Vec<ResultRecord>> {
        let query = StatementQuery {
            classes: vec![RESULT_CLASS.to_string()],
            scopes: scopes.to_vec(),
            limit: Some(MAX_READ),
            ..StatementQuery::default()
        };
        let mut out = Vec::new();
        for row in self.store.query(&query).await? {
            match record_from(&row) {
                Ok(r) => out.push(r),
                Err(e) => {
                    tracing::warn!(target: "shodh::research", error = %e, "unreadable result skipped")
                }
            }
        }
        Ok(out)
    }

    /// The results of one paper, split into used and waiting for review, with the last
    /// extraction report.
    pub async fn list(&self, path: &str) -> ResearchResult<PaperResults> {
        let path = canonical_file(path);
        let mut records = self.current_of(&path).await?;
        sort_records(&mut records);
        let (results, review): (Vec<_>, Vec<_>) = records
            .into_iter()
            .partition(|r| r.status == ResultStatus::Accepted);
        let db = self.db.clone();
        let key = path_key(&path);
        let report = blocking(move || db.report(&key))
            .await?
            .and_then(|json| serde_json::from_str::<ExtractionReport>(&json).ok());
        Ok(PaperResults {
            results,
            review,
            report,
        })
    }

    /// Accepts (the user confirms it; it is then used) or rejects (forgotten and not
    /// extracted again) a result.
    pub async fn review(&self, id: &str, accept: bool) -> ResearchResult<()> {
        let stored = match self.store.get(id).await {
            Ok(s) => s,
            Err(StatementError::NotFound(_)) => {
                return Err(ResearchError::NotFound(format!("No result has id `{id}`.")))
            }
            Err(e) => return Err(e.into()),
        };
        if stored.statement.class != RESULT_CLASS {
            return Err(ResearchError::NotFound(format!("No result has id `{id}`.")));
        }
        let record = record_from(&stored)?;
        if accept {
            if record.extractor == ExtractorKind::User {
                return Ok(());
            }
            let mut statement = stored.statement.clone();
            statement.id = format!("res-{}", uuid::Uuid::new_v4());
            statement.valid_from = Some(self.store.now());
            statement.ontology_version = self.version();
            if let Some(p) = statement.provenance.as_mut() {
                p.extractor = Extractor {
                    kind: ExtractorKind::User,
                    version: self.app_version.clone(),
                };
                p.confidence = 1.0;
            }
            self.store
                .put(
                    statement,
                    stored.scope.clone(),
                    PutIntent::Supersede {
                        target: stored.statement.id.clone(),
                    },
                )
                .await?;
        } else {
            let fp = record_fingerprint(&record);
            let db = self.db.clone();
            let key = path_key(&record.file_path);
            blocking(move || db.reject(&fp, &key)).await?;
            self.store.forget(id).await?;
        }
        Ok(())
    }

    /// A comparison of accepted results matching `filter`, with coverage notes.
    pub async fn query(&self, filter: &ResultFilter) -> ResearchResult<Comparison> {
        let records = self.all(&filter.scopes).await?;
        let papers_filter: HashSet<String> = filter.papers.iter().map(|p| path_key(p)).collect();
        let want = |kind: &str, value: &Option<String>| -> Option<(String, String)> {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| {
                    let id = if v.starts_with(&format!("{kind}:")) {
                        v.to_lowercase()
                    } else {
                        canonical_id(kind, v)
                    };
                    (id, v.to_string())
                })
        };
        let method = want("method", &filter.method);
        let dataset = want("dataset", &filter.dataset);
        let metric = want("metric", &filter.metric);
        let matches = |r: &ResultRecord| {
            method.as_ref().is_none_or(|(id, _)| &r.method_id == id)
                && dataset.as_ref().is_none_or(|(id, _)| &r.dataset_id == id)
                && metric.as_ref().is_none_or(|(id, _)| &r.metric_id == id)
                && (papers_filter.is_empty() || papers_filter.contains(&path_key(&r.file_path)))
        };
        let matching: Vec<&ResultRecord> = records.iter().filter(|r| matches(r)).collect();
        let pending_review = matching
            .iter()
            .filter(|r| r.status == ResultStatus::Review)
            .count();
        let mut accepted: Vec<&ResultRecord> = matching
            .into_iter()
            .filter(|r| r.status == ResultStatus::Accepted)
            .collect();
        accepted.sort_by(|a, b| {
            a.method_id
                .cmp(&b.method_id)
                .then(a.file_path.cmp(&b.file_path))
                .then(a.page.cmp(&b.page))
        });

        let mut columns: Vec<ComparisonColumn> = Vec::new();
        let mut rows: Vec<ComparisonRow> = Vec::new();
        let mut row_index: HashMap<String, usize> = HashMap::new();
        let mut column_papers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut papers: BTreeMap<String, String> = BTreeMap::new();
        let mut truncated = false;
        // Column and row labels: the label printed most often (ties: the shorter, then
        // the alphabetically first), so the table reads the same whatever the order.
        let mut dataset_labels: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
        let mut metric_labels: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
        let mut method_labels: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
        for r in &accepted {
            *dataset_labels
                .entry(&r.dataset_id)
                .or_default()
                .entry(&r.dataset)
                .or_default() += 1;
            *metric_labels
                .entry(&r.metric_id)
                .or_default()
                .entry(&r.metric)
                .or_default() += 1;
            *method_labels
                .entry(&r.method_id)
                .or_default()
                .entry(&r.method)
                .or_default() += 1;
        }
        let label = |labels: &HashMap<&str, HashMap<&str, usize>>, id: &str| -> String {
            labels
                .get(id)
                .and_then(|counts| {
                    counts.iter().max_by(|a, b| {
                        a.1.cmp(b.1)
                            .then(b.0.len().cmp(&a.0.len()))
                            .then(b.0.cmp(a.0))
                    })
                })
                .map(|(l, _)| l.to_string())
                .unwrap_or_else(|| id.to_string())
        };
        for r in &accepted {
            let key = format!("{}|{}", r.dataset_id, r.metric_id);
            if !columns.iter().any(|c| c.key == key) {
                columns.push(ComparisonColumn {
                    key: key.clone(),
                    dataset: label(&dataset_labels, &r.dataset_id),
                    metric: label(&metric_labels, &r.metric_id),
                });
            }
            let index = match row_index.get(&r.method_id) {
                Some(i) => *i,
                None => {
                    if rows.len() == MAX_COMPARISON_ROWS {
                        truncated = true;
                        continue;
                    }
                    rows.push(ComparisonRow {
                        method: label(&method_labels, &r.method_id),
                        method_id: r.method_id.clone(),
                        cells: BTreeMap::new(),
                    });
                    row_index.insert(r.method_id.clone(), rows.len() - 1);
                    rows.len() - 1
                }
            };
            rows[index]
                .cells
                .entry(key.clone())
                .or_default()
                .push(ComparisonCell {
                    result_id: r.id.clone(),
                    value: r.value.clone(),
                    value_text: r.value_text.clone(),
                    unit: r.unit.clone(),
                    spread: r.spread.clone(),
                    file_path: r.file_path.clone(),
                    file_name: r.file_name.clone(),
                    page: r.page,
                    region: r.region,
                    setting: r.setting.clone(),
                });
            column_papers
                .entry(key)
                .or_default()
                .insert(r.file_path.clone());
            papers.insert(r.file_path.clone(), r.file_name.clone());
        }

        // Coverage notes.
        let mut notes = Vec::new();
        for column in &columns {
            let n = column_papers
                .get(&column.key)
                .map(BTreeSet::len)
                .unwrap_or(0);
            notes.push(format!(
                "{n} {} {} {} on {}.",
                plural(n, "paper", "papers"),
                if n == 1 { "reports" } else { "report" },
                column.metric,
                column.dataset
            ));
        }
        let db = self.db.clone();
        let reports: Vec<(String, String)> = blocking(move || db.reports()).await?;
        let known: HashSet<String> = filter.known_papers.iter().map(|p| path_key(p)).collect();
        // Papers in scope that were scanned: every extracted paper, limited to the papers
        // asked for and, when the caller knows the papers in scope, to those.
        let scanned: BTreeSet<String> = reports
            .iter()
            .map(|(p, _)| p.clone())
            .filter(|p| papers_filter.is_empty() || papers_filter.contains(p))
            .filter(|p| known.is_empty() || known.contains(p))
            .collect();
        let contributing: BTreeSet<String> = papers.keys().map(|p| path_key(p)).collect();
        if metric.is_some() || dataset.is_some() || method.is_some() {
            let silent: Vec<&String> = scanned.difference(&contributing).collect();
            if !silent.is_empty() {
                let what = describe(&method, &dataset, &metric);
                notes.push(format!(
                    "{} {} in scope {} no comparable {what}: {}.",
                    silent.len(),
                    plural(silent.len(), "paper", "papers"),
                    if silent.len() == 1 {
                        "reports"
                    } else {
                        "report"
                    },
                    name_list(&silent)
                ));
            }
        }
        let unscanned: Vec<String> = filter
            .known_papers
            .iter()
            .filter(|p| papers_filter.is_empty() || papers_filter.contains(&path_key(p)))
            .filter(|p| !reports.iter().any(|(r, _)| *r == path_key(p)))
            .map(|p| canonical_file(p))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !unscanned.is_empty() {
            let refs: Vec<&String> = unscanned.iter().collect();
            notes.push(format!(
                "{} {} in scope {} not been scanned for results yet: {}.",
                unscanned.len(),
                plural(unscanned.len(), "paper", "papers"),
                if unscanned.len() == 1 { "has" } else { "have" },
                name_list(&refs)
            ));
        }
        if pending_review > 0 {
            notes.push(format!(
                "{pending_review} matching {} {} review and {} not included.",
                plural(pending_review, "value", "values"),
                if pending_review == 1 {
                    "awaits"
                } else {
                    "await"
                },
                if pending_review == 1 { "is" } else { "are" },
            ));
        }
        if accepted.is_empty() {
            notes.insert(
                0,
                format!(
                    "No accepted results match {}.",
                    describe(&method, &dataset, &metric)
                ),
            );
        }
        if truncated {
            notes.push(format!(
                "Only the first {MAX_COMPARISON_ROWS} methods are shown."
            ));
        }
        Ok(Comparison {
            columns,
            rows,
            papers: papers
                .into_iter()
                .map(|(file_path, file_name)| PaperRef {
                    file_path,
                    file_name,
                })
                .collect(),
            notes,
            pending_review,
            truncated,
        })
    }

    /// Methods, datasets, metrics and papers of accepted results.
    pub async fn facets(&self, scopes: &[Scope]) -> ResearchResult<ResultFacets> {
        let records = self.all(scopes).await?;
        let accepted: Vec<&ResultRecord> = records
            .iter()
            .filter(|r| r.status == ResultStatus::Accepted)
            .collect();
        let facet = |pick: &dyn Fn(&ResultRecord) -> (&str, &str)| -> Vec<FacetValue> {
            let mut counts: BTreeMap<String, (HashMap<String, usize>, usize)> = BTreeMap::new();
            for r in &accepted {
                let (id, label) = pick(r);
                let entry = counts.entry(id.to_string()).or_default();
                *entry.0.entry(label.to_string()).or_default() += 1;
                entry.1 += 1;
            }
            let mut out: Vec<FacetValue> = counts
                .into_iter()
                .map(|(id, (labels, count))| FacetValue {
                    label: labels
                        .into_iter()
                        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                        .map(|(l, _)| l)
                        .unwrap_or_else(|| id.clone()),
                    id,
                    count,
                })
                .collect();
            out.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
            out
        };
        let mut papers: BTreeMap<String, usize> = BTreeMap::new();
        for r in &accepted {
            *papers.entry(r.file_path.clone()).or_default() += 1;
        }
        Ok(ResultFacets {
            methods: facet(&|r| (r.method_id.as_str(), r.method.as_str())),
            datasets: facet(&|r| (r.dataset_id.as_str(), r.dataset.as_str())),
            metrics: facet(&|r| (r.metric_id.as_str(), r.metric.as_str())),
            papers: papers
                .into_iter()
                .map(|(file_path, results)| FacetPaper {
                    file_name: file_name(&file_path),
                    file_path,
                    results,
                })
                .collect(),
        })
    }
}

async fn existing_by_user(store: &StatementStore, id: &str) -> ResearchResult<bool> {
    Ok(store
        .get(id)
        .await?
        .statement
        .provenance
        .as_ref()
        .is_some_and(|p| p.extractor.kind == ExtractorKind::User))
}

fn sort_records(records: &mut [ResultRecord]) {
    records.sort_by(|a, b| {
        a.page
            .cmp(&b.page)
            .then(
                b.region
                    .map(|r| r.y1)
                    .unwrap_or(0.0)
                    .total_cmp(&a.region.map(|r| r.y1).unwrap_or(0.0)),
            )
            .then(
                a.region
                    .map(|r| r.x0)
                    .unwrap_or(0.0)
                    .total_cmp(&b.region.map(|r| r.x0).unwrap_or(0.0)),
            )
            .then(a.method.cmp(&b.method))
    });
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

fn describe(
    method: &Option<(String, String)>,
    dataset: &Option<(String, String)>,
    metric: &Option<(String, String)>,
) -> String {
    let mut text = metric
        .as_ref()
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| "metric".to_string());
    if let Some((_, d)) = dataset {
        text.push_str(&format!(" on {d}"));
    }
    if let Some((_, m)) = method {
        text.push_str(&format!(" for {m}"));
    }
    text
}

fn name_list(paths: &[&String]) -> String {
    let mut names: Vec<String> = paths
        .iter()
        .take(NAMED_PAPERS)
        .map(|p| file_name(p))
        .collect();
    if paths.len() > NAMED_PAPERS {
        names.push(format!("and {} more", paths.len() - NAMED_PAPERS));
    }
    names.join(", ")
}

#[cfg(test)]
mod tests;
