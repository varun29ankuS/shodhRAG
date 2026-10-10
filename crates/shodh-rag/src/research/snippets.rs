//! Snippets: saved regions of PDF pages as research-pack `Snippet` statements.
//!
//! A snippet is an entity (`snippet:<uuid>`, the statement subject) whose current state is
//! one `Snippet` statement. Edits (title, note, tags, kind, the image an agent-made
//! snippet gets on first display, a LaTeX transcription) store a new statement that
//! supersedes the current one, so the id stays and the history is kept. The creation time
//! is the provenance's `extracted_at`; the last change is the current statement's
//! `valid_from`. Deleting forgets every version and drops the image when no other snippet
//! uses it.
//!
//! The statement's text rendering (embedded for semantic search) is the snippet's title,
//! text and note (see `statements::render`).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shodh_ontology::{EntityRef, Extractor, ExtractorKind, Provenance, RawValue, Statement};

use super::db::{hash_of_uri, ResearchDb};
use super::pdf_text::PageRect;
use super::{blocking, canonical_file, document_entity, file_name, ResearchError, ResearchResult};
use crate::processing::document_model::{BBox, BlockKind};
use crate::statements::{
    PropertyFilter, PutIntent, Scope, StatementQuery, StatementStore, StoredStatement,
};

/// Class of snippet statements.
pub const SNIPPET_CLASS: &str = "Snippet";
/// Longest snippet text kept, in characters.
pub const MAX_TEXT_CHARS: usize = 20_000;
/// Longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest note, in characters.
pub const MAX_NOTE_CHARS: usize = 4_000;
/// Most tags on one snippet, and the longest tag.
pub const MAX_TAGS: usize = 20;
pub const MAX_TAG_CHARS: usize = 60;
/// Longest LaTeX transcription, in characters.
pub const MAX_LATEX_CHARS: usize = 8_000;
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1_000;

/// What a snippet shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnippetKind {
    Figure,
    Table,
    Equation,
    #[default]
    Passage,
}

impl SnippetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SnippetKind::Figure => "figure",
            SnippetKind::Table => "table",
            SnippetKind::Equation => "equation",
            SnippetKind::Passage => "passage",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "figure" => Some(SnippetKind::Figure),
            "table" => Some(SnippetKind::Table),
            "equation" => Some(SnippetKind::Equation),
            "passage" => Some(SnippetKind::Passage),
            _ => None,
        }
    }
}

/// Who made a snippet: the user in the viewer, or the agent (with the user's approval).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnippetAuthor {
    User,
    Agent { model: String },
}

/// A new snippet.
#[derive(Debug, Clone)]
pub struct NewSnippet {
    pub file_path: String,
    /// 1-based page.
    pub page: u32,
    pub rect: PageRect,
    pub text: String,
    /// PNG bytes; agent-made snippets have none until the viewer renders them.
    pub image_png: Option<Vec<u8>>,
    pub title: Option<String>,
    pub note: Option<String>,
    pub tags: Vec<String>,
    pub kind: SnippetKind,
    pub scope: Scope,
    pub author: SnippetAuthor,
}

/// A change to a snippet; `None` keeps the field.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub kind: Option<SnippetKind>,
}

/// Which snippets to list.
#[derive(Debug, Clone, Default)]
pub struct SnippetQuery {
    /// Only snippets of this file.
    pub file_path: Option<String>,
    /// Hybrid (semantic + keyword) search over title, text and note.
    pub text: Option<String>,
    /// Only these scopes; empty means every scope.
    pub scopes: Vec<Scope>,
    pub limit: Option<usize>,
}

/// A snippet as the app shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    /// Stable id (`snippet:<uuid>`).
    pub id: String,
    /// The current statement version.
    pub statement_id: String,
    pub file_path: String,
    pub file_name: String,
    pub page: u32,
    pub rect: PageRect,
    pub text: String,
    pub title: String,
    pub note: String,
    pub tags: Vec<String>,
    pub kind: SnippetKind,
    pub has_image: bool,
    pub latex: Option<String>,
    pub latex_model: Option<String>,
    pub scope: Scope,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Rows of the parser's table block under a snippet.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetTable {
    pub caption: Option<String>,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub page: u32,
}

fn text_of(properties: &BTreeMap<String, RawValue>, key: &str) -> Option<String> {
    match properties.get(key)? {
        RawValue::Text(t) => Some(t.clone()),
        RawValue::List(items) => items.iter().find_map(|i| match i {
            RawValue::Text(t) => Some(t.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn texts_of(properties: &BTreeMap<String, RawValue>, key: &str) -> Vec<String> {
    match properties.get(key) {
        Some(RawValue::Text(t)) => vec![t.clone()],
        Some(RawValue::List(items)) => items
            .iter()
            .filter_map(|i| match i {
                RawValue::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn integer_of(properties: &BTreeMap<String, RawValue>, key: &str) -> Option<i64> {
    match properties.get(key)? {
        RawValue::Integer(i) => Some(*i),
        RawValue::Text(t) => t.parse().ok(),
        _ => None,
    }
}

fn clean_line(text: &str, max: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

fn clean_block(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
}

fn clean_tags(tags: &[String]) -> ResearchResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = clean_line(tag, MAX_TAG_CHARS);
        if !tag.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            out.push(tag);
        }
    }
    if out.len() > MAX_TAGS {
        return Err(ResearchError::Invalid(format!(
            "A snippet can have at most {MAX_TAGS} tags."
        )));
    }
    Ok(out)
}

/// Decodes a stored `Snippet` statement.
pub fn snippet_from(stored: &StoredStatement) -> ResearchResult<Snippet> {
    let s = &stored.statement;
    let corrupt =
        |what: &str| ResearchError::Database(format!("snippet statement `{}` has no {what}", s.id));
    let subject = s.subject.as_ref().ok_or_else(|| corrupt("subject"))?;
    let provenance = s.provenance.as_ref().ok_or_else(|| corrupt("provenance"))?;
    let p = &s.properties;
    let page = integer_of(p, "snippetPage")
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| corrupt("page"))?;
    let rect = text_of(p, "snippetRect")
        .and_then(|r| PageRect::from_property(&r))
        .ok_or_else(|| corrupt("rectangle"))?;
    Ok(Snippet {
        id: subject.id.clone(),
        statement_id: s.id.clone(),
        file_path: provenance.source.clone(),
        file_name: file_name(&provenance.source),
        page,
        rect,
        text: text_of(p, "snippetText").unwrap_or_default(),
        title: text_of(p, "snippetTitle").unwrap_or_default(),
        note: text_of(p, "snippetNote").unwrap_or_default(),
        tags: texts_of(p, "snippetTag"),
        kind: text_of(p, "snippetKind")
            .and_then(|k| SnippetKind::parse(&k))
            .unwrap_or_default(),
        has_image: text_of(p, "snippetImage")
            .as_deref()
            .and_then(hash_of_uri)
            .is_some(),
        latex: text_of(p, "snippetLatex"),
        latex_model: text_of(p, "snippetLatexModel"),
        scope: stored.scope.clone(),
        created_at: provenance.extracted_at,
        updated_at: stored.valid_from,
    })
}

/// The snippet layer over the statement store and `shodh.db`.
pub struct SnippetService {
    store: Arc<StatementStore>,
    db: Arc<ResearchDb>,
    app_version: String,
}

impl std::fmt::Debug for SnippetService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnippetService").finish_non_exhaustive()
    }
}

impl SnippetService {
    pub fn new(store: Arc<StatementStore>, db: Arc<ResearchDb>, app_version: &str) -> Self {
        Self {
            store,
            db,
            app_version: app_version.to_string(),
        }
    }

    /// The research tables of `shodh.db`.
    pub fn db(&self) -> &Arc<ResearchDb> {
        &self.db
    }

    fn version(&self) -> semver::Version {
        let ontology = self.store.ontology();
        ontology
            .class(SNIPPET_CLASS)
            .and_then(|c| ontology.source(&c.source))
            .map(|s| s.version.clone())
            .unwrap_or_else(|| ontology.version().clone())
    }

    /// Saves a new snippet.
    pub async fn create(&self, new: NewSnippet) -> ResearchResult<Snippet> {
        new.rect.validate()?;
        if new.page == 0 {
            return Err(ResearchError::Invalid("Pages start at 1.".to_string()));
        }
        let path = canonical_file(&new.file_path);
        if path.is_empty() {
            return Err(ResearchError::Invalid(
                "The snippet needs a file.".to_string(),
            ));
        }
        let image = match new.image_png {
            Some(png) => {
                let db = self.db.clone();
                Some(blocking(move || db.put_image(&png)).await?)
            }
            None => None,
        };
        let now = self.store.now();
        let mut properties: BTreeMap<String, RawValue> = BTreeMap::new();
        properties.insert(
            "snippetOf".to_string(),
            RawValue::Entity(document_entity(&path)),
        );
        properties.insert(
            "snippetPage".to_string(),
            RawValue::Integer(i64::from(new.page)),
        );
        properties.insert(
            "snippetRect".to_string(),
            RawValue::text(new.rect.to_property()),
        );
        properties.insert("snippetKind".to_string(), RawValue::text(new.kind.as_str()));
        let text = clean_block(&new.text, MAX_TEXT_CHARS);
        if !text.is_empty() {
            properties.insert("snippetText".to_string(), RawValue::text(text));
        }
        if let Some(title) = new.title.as_deref().map(|t| clean_line(t, MAX_TITLE_CHARS)) {
            if !title.is_empty() {
                properties.insert("snippetTitle".to_string(), RawValue::text(title));
            }
        }
        if let Some(note) = new.note.as_deref().map(|n| clean_block(n, MAX_NOTE_CHARS)) {
            if !note.is_empty() {
                properties.insert("snippetNote".to_string(), RawValue::text(note));
            }
        }
        let tags = clean_tags(&new.tags)?;
        if !tags.is_empty() {
            properties.insert(
                "snippetTag".to_string(),
                RawValue::List(tags.into_iter().map(RawValue::Text).collect()),
            );
        }
        if let Some(image) = &image {
            properties.insert("snippetImage".to_string(), RawValue::text(image.uri()));
        }
        let extractor = match &new.author {
            SnippetAuthor::User => Extractor {
                kind: ExtractorKind::User,
                version: self.app_version.clone(),
            },
            SnippetAuthor::Agent { model } => Extractor {
                kind: ExtractorKind::Llm,
                version: format!("agent/{model}"),
            },
        };
        let entity = format!("snippet:{}", uuid::Uuid::new_v4());
        let statement = Statement {
            id: format!("snip-{}", uuid::Uuid::new_v4()),
            class: SNIPPET_CLASS.to_string(),
            subject: Some(EntityRef::typed(entity.clone(), SNIPPET_CLASS)),
            properties,
            ontology_version: self.version(),
            valid_from: Some(now),
            provenance: Some(Provenance {
                source: path,
                generation: 0,
                page: Some(new.page),
                span: None,
                extractor,
                confidence: 1.0,
                extracted_at: now,
            }),
        };
        self.store
            .put(statement, new.scope, PutIntent::Auto)
            .await?;
        self.get(&entity).await
    }

    async fn current(&self, id: &str) -> ResearchResult<StoredStatement> {
        let id = id.trim();
        if !id.starts_with("snippet:") {
            return Err(ResearchError::NotFound(format!(
                "No snippet has id `{id}`."
            )));
        }
        let query = StatementQuery {
            classes: vec![SNIPPET_CLASS.to_string()],
            subject: Some(id.to_string()),
            limit: Some(1),
            ..StatementQuery::default()
        };
        self.store
            .query(&query)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| ResearchError::NotFound(format!("No snippet has id `{id}`.")))
    }

    /// One snippet by id.
    pub async fn get(&self, id: &str) -> ResearchResult<Snippet> {
        snippet_from(&self.current(id).await?)
    }

    /// Snippets matching `query`: newest first, or best match first with `text`.
    pub async fn list(&self, query: &SnippetQuery) -> ResearchResult<Vec<Snippet>> {
        let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let file = query
            .file_path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(canonical_file);
        let mut statements = StatementQuery {
            classes: vec![SNIPPET_CLASS.to_string()],
            scopes: query.scopes.clone(),
            limit: Some(MAX_LIMIT),
            ..StatementQuery::default()
        };
        if let Some(path) = &file {
            statements.properties.push(PropertyFilter {
                property: "snippetOf".to_string(),
                equals: format!("@{}", document_entity(path).id),
            });
        }
        let rows: Vec<StoredStatement> = match query
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            Some(text) => self
                .store
                .search(text, &statements, limit)
                .await?
                .into_iter()
                .map(|hit| hit.stored)
                .collect(),
            None => self.store.query(&statements).await?,
        };
        let mut out = Vec::new();
        for row in rows.iter().take(limit) {
            match snippet_from(row) {
                Ok(snippet) => out.push(snippet),
                Err(e) => {
                    tracing::warn!(target: "shodh::research", error = %e, "unreadable snippet skipped")
                }
            }
        }
        Ok(out)
    }

    /// Stores a new version of the snippet with `change` applied to its properties.
    async fn revise(
        &self,
        id: &str,
        change: impl FnOnce(&mut BTreeMap<String, RawValue>) -> ResearchResult<()>,
    ) -> ResearchResult<Snippet> {
        let current = self.current(id).await?;
        let mut statement = current.statement.clone();
        change(&mut statement.properties)?;
        if statement.properties == current.statement.properties {
            return snippet_from(&current);
        }
        statement.id = format!("snip-{}", uuid::Uuid::new_v4());
        statement.ontology_version = self.version();
        statement.valid_from = Some(self.store.now());
        self.store
            .put(
                statement,
                current.scope.clone(),
                PutIntent::Supersede {
                    target: current.statement.id.clone(),
                },
            )
            .await?;
        self.get(id).await
    }

    /// Changes title, note, tags or kind.
    pub async fn update(&self, id: &str, patch: SnippetPatch) -> ResearchResult<Snippet> {
        let tags = patch.tags.as_deref().map(clean_tags).transpose()?;
        self.revise(id, move |p| {
            if let Some(title) = patch.title.as_deref() {
                set_text(p, "snippetTitle", clean_line(title, MAX_TITLE_CHARS));
            }
            if let Some(note) = patch.note.as_deref() {
                set_text(p, "snippetNote", clean_block(note, MAX_NOTE_CHARS));
            }
            if let Some(tags) = tags {
                if tags.is_empty() {
                    p.remove("snippetTag");
                } else {
                    p.insert(
                        "snippetTag".to_string(),
                        RawValue::List(tags.into_iter().map(RawValue::Text).collect()),
                    );
                }
            }
            if let Some(kind) = patch.kind {
                p.insert("snippetKind".to_string(), RawValue::text(kind.as_str()));
            }
            Ok(())
        })
        .await
    }

    /// Attaches the rendered image (agent-made snippets get it on first display).
    pub async fn set_image(&self, id: &str, png: Vec<u8>) -> ResearchResult<Snippet> {
        let db = self.db.clone();
        let image = blocking(move || db.put_image(&png)).await?;
        let uri = image.uri();
        self.revise(id, move |p| {
            p.insert("snippetImage".to_string(), RawValue::text(uri));
            Ok(())
        })
        .await
    }

    /// The PNG of a snippet, when it has one.
    pub async fn image(&self, id: &str) -> ResearchResult<Option<Vec<u8>>> {
        let current = self.current(id).await?;
        let Some(hash) = text_of(&current.statement.properties, "snippetImage")
            .as_deref()
            .and_then(hash_of_uri)
            .map(str::to_string)
        else {
            return Ok(None);
        };
        let db = self.db.clone();
        blocking(move || db.image(&hash)).await
    }

    /// Stores a LaTeX transcription and the model that made it.
    pub async fn set_latex(&self, id: &str, latex: &str, model: &str) -> ResearchResult<Snippet> {
        let latex = clean_block(latex, MAX_LATEX_CHARS);
        if latex.is_empty() {
            return Err(ResearchError::Model(
                "The model returned no LaTeX.".to_string(),
            ));
        }
        let model = clean_line(model, MAX_TITLE_CHARS);
        self.revise(id, move |p| {
            p.insert("snippetLatex".to_string(), RawValue::text(latex));
            p.insert("snippetLatexModel".to_string(), RawValue::text(model));
            Ok(())
        })
        .await
    }

    /// Deletes a snippet: every version is forgotten and the image dropped when no
    /// current snippet still shows it.
    pub async fn delete(&self, id: &str) -> ResearchResult<Snippet> {
        let current = self.current(id).await?;
        let snippet = snippet_from(&current)?;
        let image = text_of(&current.statement.properties, "snippetImage");
        for version in self.store.history_ids(&current.statement.id).await? {
            match self.store.forget(&version).await {
                Ok(_) | Err(crate::statements::StatementError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(uri) = image {
            let still_used = !self
                .store
                .query(&StatementQuery {
                    classes: vec![SNIPPET_CLASS.to_string()],
                    properties: vec![PropertyFilter {
                        property: "snippetImage".to_string(),
                        equals: uri.clone(),
                    }],
                    limit: Some(1),
                    ..StatementQuery::default()
                })
                .await?
                .is_empty();
            if let (false, Some(hash)) = (still_used, hash_of_uri(&uri).map(str::to_string)) {
                let db = self.db.clone();
                blocking(move || db.delete_image(&hash)).await?;
            }
        }
        Ok(snippet)
    }

    /// The rows of the parser's table block under the snippet, if the PDF has one there.
    pub async fn table(&self, id: &str) -> ResearchResult<Option<SnippetTable>> {
        let snippet = self.get(id).await?;
        let path = snippet.file_path.clone();
        blocking(move || {
            let bytes = std::fs::read(&path).map_err(|e| {
                ResearchError::Pdf(format!("{} could not be read: {e}", file_name(&path)))
            })?;
            let doc = crate::processing::pdf_layout::parse_pdf_layout(&bytes)
                .map_err(|e| ResearchError::Pdf(format!("The PDF could not be parsed: {e}")))?;
            Ok(table_under(&doc, snippet.page, snippet.rect))
        })
        .await
    }
}

fn set_text(properties: &mut BTreeMap<String, RawValue>, key: &str, value: String) {
    if value.is_empty() {
        properties.remove(key);
    } else {
        properties.insert(key.to_string(), RawValue::text(value));
    }
}

/// The table block on `page` that overlaps `rect` most (at least half of the rectangle
/// or a quarter of the table), restricted to the body rows whose cells lie inside the
/// rectangle when the table has cell boxes and the rectangle covers only part of it.
pub fn table_under(
    doc: &crate::processing::document_model::StructuredDocument,
    page: u32,
    rect: PageRect,
) -> Option<SnippetTable> {
    let info = doc.pages.iter().find(|p| p.number == page)?;
    let user = rect.to_user_space((0.0, 0.0, info.width, info.height));
    let overlap = |b: &BBox| {
        let w = b.x1.min(user.x1) - b.x0.max(user.x0);
        let h = b.y1.min(user.y1) - b.y0.max(user.y0);
        if w <= 0.0 || h <= 0.0 {
            0.0
        } else {
            w * h
        }
    };
    let rect_area = user.width() * user.height();
    let best = doc
        .blocks
        .iter()
        .filter(|b| b.page == Some(page))
        .filter_map(|b| match (&b.kind, b.bbox) {
            (BlockKind::Table { .. }, Some(bbox)) => Some((b, bbox, overlap(&bbox))),
            _ => None,
        })
        .filter(|(_, bbox, o)| {
            *o > 0.0 && (*o >= 0.5 * rect_area || *o >= 0.25 * bbox.width() * bbox.height())
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))?;
    let (block, bbox, _) = best;
    let BlockKind::Table {
        header,
        rows,
        caption,
        cell_boxes,
        ..
    } = &block.kind
    else {
        return None;
    };
    // The rectangle covers the whole table (all but a sliver of it).
    let whole = overlap(&bbox) >= 0.9 * bbox.width() * bbox.height();
    let body: Vec<Vec<String>> = if whole || cell_boxes.len() != rows.len() + 1 {
        rows.clone()
    } else {
        rows.iter()
            .zip(cell_boxes.iter().skip(1))
            .filter(|(_, boxes)| {
                boxes
                    .iter()
                    .flatten()
                    .any(|b| user.contains_point(b.center_x(), b.center_y()))
            })
            .map(|(row, _)| row.clone())
            .collect()
    };
    Some(SnippetTable {
        caption: caption.clone(),
        header: header.clone(),
        rows: body,
        page,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::document_model::{Block, PageInfo, StructuredDocument};
    use crate::research::db::tests::tiny_png;
    use crate::statements::testing::{t0, FixedEmbedder, TestClock, WordEmbedder};
    use crate::statements::DynamicsStore;

    async fn service(dir: &std::path::Path) -> SnippetService {
        service_with_clock(dir, TestClock::at(t0())).await
    }

    async fn service_with_clock(dir: &std::path::Path, clock: Arc<TestClock>) -> SnippetService {
        let ontology =
            Arc::new(shodh_ontology::Ontology::builtin_with_packs(&["research"]).unwrap());
        let dynamics = Arc::new(DynamicsStore::open(&dir.join("shodh.db"), None).unwrap());
        let store = StatementStore::open(
            &dir.join("lance"),
            crate::statements::testing::DIM,
            ontology,
            dynamics,
            Arc::new(FixedEmbedder(Arc::new(WordEmbedder::default()))),
            clock,
        )
        .await
        .unwrap();
        let db = Arc::new(ResearchDb::open(&dir.join("shodh.db"), None).unwrap());
        SnippetService::new(Arc::new(store), db, "test")
    }

    fn new_snippet(path: &str, text: &str, image: Option<Vec<u8>>) -> NewSnippet {
        NewSnippet {
            file_path: path.to_string(),
            page: 3,
            rect: PageRect {
                x: 72.0,
                y: 100.5,
                width: 200.0,
                height: 40.0,
            },
            text: text.to_string(),
            image_png: image,
            title: Some("  Loss   function ".to_string()),
            note: None,
            tags: vec!["loss".into(), "Loss".into(), " eq ".into()],
            kind: SnippetKind::Equation,
            scope: Scope::Workspace("source-1".into()),
            author: SnippetAuthor::User,
        }
    }

    #[tokio::test]
    async fn a_snippet_round_trips_through_the_statement_store() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::at(t0());
        let service = service_with_clock(dir.path(), clock.clone()).await;
        let png = tiny_png();
        let created = service
            .create(new_snippet(
                "C:/papers/attention.pdf",
                "The loss is the negative log likelihood",
                Some(png.clone()),
            ))
            .await
            .unwrap();
        assert!(created.id.starts_with("snippet:"));
        assert_eq!(created.page, 3);
        assert_eq!(created.rect.to_property(), "72,100.5,200,40");
        assert_eq!(created.title, "Loss function");
        assert_eq!(created.tags, vec!["loss".to_string(), "eq".to_string()]);
        assert_eq!(created.kind, SnippetKind::Equation);
        assert!(created.has_image);
        assert_eq!(created.file_name, "attention.pdf");
        assert_eq!(created.scope, Scope::Workspace("source-1".into()));
        assert_eq!(service.image(&created.id).await.unwrap(), Some(png.clone()));

        // Edits keep the id and supersede the statement.
        clock.advance_days(1);
        let edited = service
            .update(
                &created.id,
                SnippetPatch {
                    note: Some("from section 3".into()),
                    tags: Some(vec![]),
                    ..SnippetPatch::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(edited.id, created.id);
        assert_ne!(edited.statement_id, created.statement_id);
        assert_eq!(edited.note, "from section 3");
        assert!(edited.tags.is_empty());
        assert_eq!(edited.created_at, created.created_at);
        assert!(edited.updated_at > created.updated_at);
        assert_eq!(edited.title, "Loss function");
        clock.advance_days(1);
        let latex = service
            .set_latex(&created.id, "\\mathcal{L} = -\\log p(x)", "qwen2.5-vl")
            .await
            .unwrap();
        assert_eq!(latex.latex_model.as_deref(), Some("qwen2.5-vl"));

        // Listing: per file, by meaning, by scope.
        service
            .create(new_snippet(
                "C:/papers/other.pdf",
                "Figure of the encoder stack",
                None,
            ))
            .await
            .unwrap();
        let of_file = service
            .list(&SnippetQuery {
                file_path: Some("C:\\papers\\attention.pdf".into()),
                ..SnippetQuery::default()
            })
            .await
            .unwrap();
        if cfg!(windows) {
            assert_eq!(of_file.len(), 1);
            assert_eq!(of_file[0].id, created.id);
        }
        let found = service
            .list(&SnippetQuery {
                text: Some("encoder".into()),
                limit: Some(1),
                ..SnippetQuery::default()
            })
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file_name, "other.pdf");
        assert!(!found[0].has_image);
        let elsewhere = service
            .list(&SnippetQuery {
                scopes: Scope::Workspace("source-2".into()).visible(),
                ..SnippetQuery::default()
            })
            .await
            .unwrap();
        assert!(elsewhere.is_empty());

        // Agent-made snippets get their image later.
        clock.advance_days(1);
        let later = service.set_image(&found[0].id, png.clone()).await.unwrap();
        assert!(later.has_image);

        // Deleting forgets every version; the shared image survives while used.
        service.delete(&created.id).await.unwrap();
        assert!(matches!(
            service.get(&created.id).await,
            Err(ResearchError::NotFound(_))
        ));
        assert_eq!(service.image(&later.id).await.unwrap(), Some(png.clone()));
        service.delete(&later.id).await.unwrap();
        let hash = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&png));
        assert_eq!(service.db().image(&hash).unwrap(), None);
        assert!(service
            .list(&SnippetQuery::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn invalid_snippets_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path()).await;
        let mut bad = new_snippet("C:/papers/a.pdf", "x", None);
        bad.rect.width = 0.0;
        assert!(matches!(
            service.create(bad).await,
            Err(ResearchError::Invalid(_))
        ));
        let mut bad = new_snippet("C:/papers/a.pdf", "x", Some(b"not a png".to_vec()));
        bad.page = 1;
        assert!(matches!(
            service.create(bad).await,
            Err(ResearchError::Invalid(_))
        ));
        let mut bad = new_snippet("C:/papers/a.pdf", "x", None);
        bad.tags = (0..30).map(|i| format!("t{i}")).collect();
        assert!(matches!(
            service.create(bad).await,
            Err(ResearchError::Invalid(_))
        ));
        assert!(matches!(
            service.get("memory-1").await,
            Err(ResearchError::NotFound(_))
        ));
    }

    #[test]
    fn the_table_under_a_rectangle_is_found_with_its_rows() {
        let table = Block::new(
            BlockKind::Table {
                header: vec!["Method".into(), "R@10".into()],
                rows: vec![
                    vec!["HNSW".into(), "95.3".into()],
                    vec!["IVF".into(), "88.1".into()],
                ],
                caption: Some("Table 2: Recall on SIFT1M.".into()),
                cell_coverage: None,
                cell_boxes: vec![
                    vec![
                        Some(BBox::new(100.0, 700.0, 150.0, 710.0)),
                        Some(BBox::new(160.0, 700.0, 200.0, 710.0)),
                    ],
                    vec![
                        Some(BBox::new(100.0, 688.0, 150.0, 698.0)),
                        Some(BBox::new(160.0, 688.0, 200.0, 698.0)),
                    ],
                    vec![
                        Some(BBox::new(100.0, 676.0, 150.0, 686.0)),
                        Some(BBox::new(160.0, 676.0, 200.0, 686.0)),
                    ],
                ],
            },
            "",
        )
        .on_page(2, Some(BBox::new(100.0, 676.0, 200.0, 710.0)));
        let doc = StructuredDocument {
            pages: vec![PageInfo {
                number: 2,
                width: 612.0,
                height: 792.0,
            }],
            blocks: vec![table],
        };
        // Whole table (top-left 90..210 x, 80..120 y => user 672..712).
        let whole = table_under(
            &doc,
            2,
            PageRect {
                x: 90.0,
                y: 80.0,
                width: 120.0,
                height: 40.0,
            },
        )
        .unwrap();
        assert_eq!(whole.rows.len(), 2);
        assert_eq!(whole.caption.as_deref(), Some("Table 2: Recall on SIFT1M."));
        // Only the first body row (user y 686..712).
        let part = table_under(
            &doc,
            2,
            PageRect {
                x: 90.0,
                y: 80.0,
                width: 120.0,
                height: 26.0,
            },
        )
        .unwrap();
        assert_eq!(
            part.rows,
            vec![vec!["HNSW".to_string(), "95.3".to_string()]]
        );
        assert!(table_under(
            &doc,
            2,
            PageRect {
                x: 300.0,
                y: 300.0,
                width: 50.0,
                height: 50.0
            }
        )
        .is_none());
        assert!(table_under(
            &doc,
            5,
            PageRect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0
            }
        )
        .is_none());
    }
}
