//! `export_document` (write): save the model's markdown, with its `[n]`
//! citations resolved into a Sources section, as Markdown or Word (DOCX).
//!
//! Where files go:
//! - by default `Documents/Shodh exports/` (created when missing);
//! - an existing absolute folder the model names otherwise;
//! - never over an existing file: names get ` (2)`, ` (3)` … and every
//!   create is exclusive;
//! - inside an indexed source folder only with the user's approval of that
//!   exact path ([`HostTool::must_confirm`]), even when writes are
//!   auto-approved; the file is written to exactly the approved path or not
//!   at all.
//!
//! PDF is not offered: the maintained pure-Rust writer (printpdf) only has
//! the 14 standard fonts, which cannot encode most non-Latin text, so a PDF
//! export would need a bundled font and a text layout engine.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use docx_rs::{
    AlignmentType, Docx, Hyperlink, HyperlinkType, Paragraph, Run, RunFonts, Style, StyleType,
    Table, TableCell, TableRow,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde_json::{json, Value};
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::{
    ApprovalPreview, CitedPassage, HostTool, RegistryError, ToolContext, ToolError, ToolOutput,
    ToolRegistry,
};
use shodh_rag::harness::RiskTier;

use super::files::{
    comparable, containing_root, create_exact, create_unique, existing_folder, next_free_path,
    sanitize_name,
};
use super::{invalid, str_arg, AgentHost};

/// Folder created under the user's Documents folder for exports.
pub const EXPORTS_FOLDER: &str = "Shodh exports";
const MAX_MARKDOWN_CHARS: usize = 200_000;

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(ExportDocumentTool {
        host: host.clone(),
        approved: Mutex::new(HashMap::new()),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Markdown,
    Docx,
}

impl Format {
    fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("md") | Some("markdown") => Format::Markdown,
            _ => Format::Docx,
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Docx => "docx",
        }
    }
}

/// One entry of the Sources section.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceEntry {
    n: u32,
    title: String,
    location: Option<String>,
    url: Option<String>,
}

impl SourceEntry {
    fn from_passage(p: &CitedPassage) -> Self {
        if p.web {
            SourceEntry {
                n: p.n,
                title: p.file.clone(),
                location: None,
                url: Some(p.path.clone()),
            }
        } else {
            let in_app = p.path.contains("://");
            SourceEntry {
                n: p.n,
                title: p.file.clone(),
                location: p
                    .page
                    .as_ref()
                    .map(|page| format!("page {page}"))
                    .or_else(|| (!in_app && p.path != p.file).then(|| p.path.clone())),
                url: None,
            }
        }
    }

    fn line(&self) -> String {
        let mut line = format!("[{}] {}", self.n, self.title);
        if let Some(location) = &self.location {
            line.push_str(&format!(", {location}"));
        }
        if let Some(url) = &self.url {
            line.push_str(&format!(" — {url}"));
        }
        line
    }
}

static CITATION: std::sync::LazyLock<Option<Regex>> =
    std::sync::LazyLock::new(|| Regex::new(r"\[(\d{1,4})\]").ok());

/// Citation numbers used in `markdown`, outside code.
fn cited_numbers(markdown: &str) -> Vec<u32> {
    let Some(re) = CITATION.as_ref() else {
        return Vec::new();
    };
    // The parser splits `[1]` into separate text events ("[", "1", "]"), so
    // prose is joined back together before matching; code is left out.
    let mut prose = String::new();
    let mut in_code = false;
    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Text(text) if !in_code => prose.push_str(&text),
            Event::Code(_) => prose.push(' '),
            Event::SoftBreak | Event::HardBreak | Event::End(_) => prose.push(' '),
            _ => {}
        }
    }
    let numbers: std::collections::BTreeSet<u32> = re
        .captures_iter(&prose)
        .filter_map(|cap| cap.get(1).and_then(|m| m.as_str().parse().ok()))
        .collect();
    numbers.into_iter().collect()
}

/// Resolve every cited number from the run's passages, then from the
/// `sources` argument (for passages from earlier answers). Unknown numbers
/// fail the export so the model fixes them instead of shipping dangling
/// citations.
fn resolve_sources(
    tool: &str,
    markdown: &str,
    args: &Value,
    ctx: &ToolContext,
) -> Result<Vec<SourceEntry>, ToolError> {
    let mut given: BTreeMap<u32, SourceEntry> = BTreeMap::new();
    for item in args
        .get("sources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(n) = item
            .get("n")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
        else {
            continue;
        };
        let title = str_arg(item, "title").unwrap_or_default().to_string();
        given.insert(
            n,
            SourceEntry {
                n,
                title,
                location: str_arg(item, "location").map(str::to_string),
                url: str_arg(item, "url").map(str::to_string),
            },
        );
    }
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for n in cited_numbers(markdown) {
        match ctx.cited_passage(n) {
            Some(p) => out.push(SourceEntry::from_passage(&p)),
            None => match given.get(&n) {
                Some(entry) if !entry.title.is_empty() => out.push(entry.clone()),
                _ => missing.push(n),
            },
        }
    }
    if !missing.is_empty() {
        let list = missing
            .iter()
            .map(|n| format!("[{n}]"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(invalid(
            tool,
            format!(
                "citations {list} do not match any passage of this answer; search again so they \
                 are numbered, or describe each in `sources` with n and title"
            ),
        ));
    }
    Ok(out)
}

// ── Rendering ──────────────────────────────────────────────────────────────

fn markdown_bytes(title: &str, markdown: &str, sources: &[SourceEntry]) -> Vec<u8> {
    let mut out = String::new();
    if !markdown.trim_start().starts_with("# ") {
        out.push_str(&format!("# {title}\n\n"));
    }
    out.push_str(markdown.trim_end());
    out.push('\n');
    if !sources.is_empty() {
        out.push_str("\n## Sources\n\n");
        for s in sources {
            out.push_str(&format!("- {}\n", s.line()));
        }
    }
    out.into_bytes()
}

#[derive(Default, Clone, Copy)]
struct Inline {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
}

// Boxed: docx-rs runs and paragraphs are large values.
enum Piece {
    Run(Box<Run>),
    Link(Box<Hyperlink>),
}

#[derive(Default)]
struct DocxBuilder {
    body: Vec<Body>,
    pieces: Vec<Piece>,
    inline: Inline,
    link: Option<(String, Vec<Run>)>,
    lists: Vec<Option<u64>>,
    heading: Option<usize>,
    quote_depth: usize,
    code_block: bool,
    table: Option<Vec<Vec<Vec<Piece>>>>,
}

enum Body {
    Paragraph(Box<Paragraph>),
    Table(Box<Table>),
}

fn styled_run(text: &str, inline: Inline) -> Run {
    let mut run = Run::new().add_text(text);
    if inline.bold {
        run = run.bold();
    }
    if inline.italic {
        run = run.italic();
    }
    if inline.strike {
        run = run.strike();
    }
    if inline.code {
        run = run.fonts(RunFonts::new().ascii("Consolas").hi_ansi("Consolas"));
    }
    run
}

impl DocxBuilder {
    fn push_text(&mut self, text: &str) {
        let run = styled_run(text, self.inline);
        match &mut self.link {
            Some((_, runs)) => runs.push(run.color("0563C1").underline("single")),
            None => self.pieces.push(Piece::Run(Box::new(run))),
        }
    }

    fn paragraph_from_pieces(&mut self) -> Paragraph {
        let mut p = Paragraph::new();
        for piece in self.pieces.drain(..) {
            p = match piece {
                Piece::Run(run) => p.add_run(*run),
                Piece::Link(link) => p.add_hyperlink(*link),
            };
        }
        p
    }

    fn flush_paragraph(&mut self) {
        if self.pieces.is_empty() {
            return;
        }
        if let Some(table) = &mut self.table {
            if let Some(row) = table.last_mut() {
                if let Some(cell) = row.last_mut() {
                    cell.append(&mut self.pieces);
                }
            }
            return;
        }
        let mut p = self.paragraph_from_pieces();
        if let Some(level) = self.heading {
            p = p.style(&format!("Heading{level}"));
        }
        let depth = self.lists.len() + self.quote_depth;
        if depth > 0 {
            let left = i32::try_from(depth * 360).unwrap_or(i32::MAX);
            p = p.indent(Some(left), None, None, None);
        }
        self.body.push(Body::Paragraph(Box::new(p)));
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.code_block {
                    for (i, line) in text.split('\n').enumerate() {
                        if i > 0 {
                            self.flush_code_line();
                        }
                        if !line.is_empty() {
                            self.push_text(line);
                        }
                    }
                } else {
                    self.push_text(&text);
                }
            }
            Event::Code(text) => {
                let saved = self.inline;
                self.inline.code = true;
                self.push_text(&text);
                self.inline = saved;
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => self.push_text(&text),
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.flush_paragraph(),
            Event::Rule => self.body.push(Body::Paragraph(Box::new(
                Paragraph::new()
                    .add_run(Run::new().add_text("———"))
                    .align(AlignmentType::Center),
            ))),
            Event::TaskListMarker(done) => self.push_text(if done { "☑ " } else { "☐ " }),
            Event::Html(html) | Event::InlineHtml(html) => self.push_text(&html),
            Event::FootnoteReference(label) => self.push_text(&format!("[^{label}]")),
        }
    }

    fn flush_code_line(&mut self) {
        let mut p = self.paragraph_from_pieces();
        p = p.indent(Some(360), None, None, None);
        self.body.push(Body::Paragraph(Box::new(p)));
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush_paragraph();
                self.heading = Some(match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    _ => 3,
                });
            }
            Tag::Paragraph => self.flush_paragraph(),
            Tag::BlockQuote(_) => {
                self.flush_paragraph();
                self.quote_depth += 1;
                self.inline.italic = true;
            }
            Tag::CodeBlock(kind) => {
                self.flush_paragraph();
                self.code_block = true;
                self.inline.code = true;
                if let CodeBlockKind::Fenced(lang) = kind {
                    if !lang.is_empty() {
                        let saved = self.inline;
                        self.inline = Inline {
                            italic: true,
                            ..Inline::default()
                        };
                        self.push_text(&format!("{lang}:"));
                        self.inline = saved;
                        self.flush_code_line();
                    }
                }
            }
            Tag::List(start) => {
                self.flush_paragraph();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush_paragraph();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let marker = format!("{n}. ");
                        *n += 1;
                        marker
                    }
                    _ => "• ".to_string(),
                };
                self.pieces
                    .push(Piece::Run(Box::new(Run::new().add_text(marker))));
            }
            Tag::Emphasis => self.inline.italic = true,
            Tag::Strong => self.inline.bold = true,
            Tag::Strikethrough => self.inline.strike = true,
            Tag::Link { dest_url, .. } => self.link = Some((dest_url.to_string(), Vec::new())),
            Tag::Table(_) => {
                self.flush_paragraph();
                self.table = Some(Vec::new());
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.push(Vec::new());
                }
                if matches!(tag, Tag::TableHead) {
                    self.inline.bold = true;
                }
            }
            Tag::TableCell => {
                if let Some(row) = self.table.as_mut().and_then(|t| t.last_mut()) {
                    row.push(Vec::new());
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.flush_paragraph();
                self.heading = None;
            }
            TagEnd::Paragraph | TagEnd::Item => self.flush_paragraph(),
            TagEnd::BlockQuote(_) => {
                self.flush_paragraph();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.inline.italic = false;
            }
            TagEnd::CodeBlock => {
                if !self.pieces.is_empty() {
                    self.flush_code_line();
                }
                self.code_block = false;
                self.inline.code = false;
            }
            TagEnd::List(_) => {
                self.flush_paragraph();
                self.lists.pop();
            }
            TagEnd::Emphasis => self.inline.italic = false,
            TagEnd::Strong => self.inline.bold = false,
            TagEnd::Strikethrough => self.inline.strike = false,
            TagEnd::Link => {
                if let Some((url, runs)) = self.link.take() {
                    let mut link = Hyperlink::new(url, HyperlinkType::External);
                    for run in runs {
                        link = link.add_run(run);
                    }
                    self.pieces.push(Piece::Link(Box::new(link)));
                }
            }
            TagEnd::TableHead => self.inline.bold = false,
            TagEnd::TableCell => {}
            TagEnd::Table => {
                if let Some(rows) = self.table.take() {
                    let rows = rows
                        .into_iter()
                        .map(|cells| {
                            TableRow::new(
                                cells
                                    .into_iter()
                                    .map(|pieces| {
                                        let mut p = Paragraph::new();
                                        for piece in pieces {
                                            p = match piece {
                                                Piece::Run(run) => p.add_run(*run),
                                                Piece::Link(link) => p.add_hyperlink(*link),
                                            };
                                        }
                                        TableCell::new().add_paragraph(p)
                                    })
                                    .collect(),
                            )
                        })
                        .collect();
                    self.body.push(Body::Table(Box::new(Table::new(rows))));
                }
            }
            _ => {}
        }
    }
}

fn heading_style(id: &str, name: &str, size: usize, level: usize) -> Style {
    Style::new(id, StyleType::Paragraph)
        .name(name)
        .size(size)
        .bold()
        .outline_lvl(level)
}

fn docx_bytes(title: &str, markdown: &str, sources: &[SourceEntry]) -> Result<Vec<u8>, ToolError> {
    let mut builder = DocxBuilder::default();
    if !markdown.trim_start().starts_with("# ") {
        builder.heading = Some(1);
        builder.push_text(title);
        builder.flush_paragraph();
        builder.heading = None;
    }
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(markdown, options) {
        builder.handle(event);
    }
    builder.flush_paragraph();

    let mut docx = Docx::new()
        .add_style(heading_style("Heading1", "Heading 1", 36, 0))
        .add_style(heading_style("Heading2", "Heading 2", 30, 1))
        .add_style(heading_style("Heading3", "Heading 3", 26, 2));
    for item in builder.body {
        docx = match item {
            Body::Paragraph(p) => docx.add_paragraph(*p),
            Body::Table(t) => docx.add_table(*t),
        };
    }
    if !sources.is_empty() {
        docx = docx.add_paragraph(
            Paragraph::new()
                .add_run(Run::new().add_text("Sources"))
                .style("Heading2"),
        );
        for s in sources {
            let mut p =
                Paragraph::new().add_run(Run::new().add_text(format!("[{}] {}", s.n, s.title)));
            if let Some(location) = &s.location {
                p = p.add_run(Run::new().add_text(format!(", {location}")));
            }
            if let Some(url) = &s.url {
                p = p.add_run(Run::new().add_text(" — ")).add_hyperlink(
                    Hyperlink::new(url.clone(), HyperlinkType::External).add_run(
                        Run::new()
                            .add_text(url.clone())
                            .color("0563C1")
                            .underline("single"),
                    ),
                );
            }
            docx = docx.add_paragraph(p);
        }
    }
    let mut bytes = Vec::new();
    docx.build()
        .pack(std::io::Cursor::new(&mut bytes))
        .map_err(|e| ToolError::Failed(format!("Building the Word document failed: {e}")))?;
    Ok(bytes)
}

// ── The tool ───────────────────────────────────────────────────────────────

pub struct ExportDocumentTool {
    host: Arc<AgentHost>,
    /// Exact paths the user approved, by step id; execute writes there or
    /// nowhere.
    approved: Mutex<HashMap<String, PathBuf>>,
}

/// Where an export goes, before any file exists.
struct Plan {
    path: PathBuf,
    folder: PathBuf,
    format: Format,
    title: String,
    /// The indexed source folder the export would land in.
    indexed_root: Option<PathBuf>,
    create_folder: bool,
}

impl ExportDocumentTool {
    async fn plan(&self, args: &Value) -> Result<Plan, ToolError> {
        let tool = app_tools::EXPORT_DOCUMENT;
        let title = str_arg(args, "title").ok_or_else(|| invalid(tool, "`title` is required"))?;
        let format = Format::parse(str_arg(args, "format"));
        let (folder, create_folder) = match str_arg(args, "folder") {
            Some(raw) => (existing_folder(tool, raw)?, false),
            None => {
                let documents = self.host.effects.documents_dir().ok_or_else(|| {
                    ToolError::Unavailable(
                        "The Documents folder could not be found; pass a folder.".to_string(),
                    )
                })?;
                let folder = documents.join(EXPORTS_FOLDER);
                let missing = !folder.is_dir();
                (folder, missing)
            }
        };
        let stem = sanitize_name(str_arg(args, "file_name").unwrap_or(title), "Shodh export");
        let stem = stem
            .strip_suffix(&format!(".{}", format.extension()))
            .map(str::to_string)
            .unwrap_or(stem);
        let path = next_free_path(&folder, &stem, format.extension())
            .map_err(|e| ToolError::Unavailable(e.to_string()))?;
        let roots = self.host.roots.folders().await?;
        let indexed_root = containing_root(&folder, &roots).cloned();
        Ok(Plan {
            path,
            folder,
            format,
            title: title.to_string(),
            indexed_root,
            create_folder,
        })
    }
}

#[async_trait]
impl HostTool for ExportDocumentTool {
    fn name(&self) -> &'static str {
        app_tools::EXPORT_DOCUMENT
    }
    fn label(&self) -> &'static str {
        "Export document"
    }
    fn label_template(&self) -> &'static str {
        "Exporting {title}[ as {format!}]"
    }
    fn description(&self) -> &'static str {
        "Save a document you wrote (markdown, with [n] citations) as Word (docx, default) or \
         Markdown (md). Citations are resolved into a Sources section listing each file and page \
         or web page; citations from earlier answers need an entry in sources. Saves to \
         Documents/Shodh exports unless you pass an existing folder; never overwrites (adds (2), \
         (3)…). Saving inside an indexed folder always asks the user."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "markdown": {"type": "string", "minLength": 1, "maxLength": MAX_MARKDOWN_CHARS},
                "format": {"type": "string", "enum": ["docx", "md"]},
                "file_name": {"type": "string", "minLength": 1, "maxLength": 200},
                "folder": {"type": "string", "minLength": 3, "maxLength": 1024},
                "sources": {
                    "type": "array",
                    "maxItems": 200,
                    "items": {
                        "type": "object",
                        "properties": {
                            "n": {"type": "integer", "minimum": 1},
                            "title": {"type": "string", "minLength": 1, "maxLength": 300},
                            "location": {"type": "string", "maxLength": 300},
                            "url": {"type": "string", "maxLength": 2048}
                        },
                        "required": ["n", "title"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["title", "markdown"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }

    async fn must_confirm(&self, args: &Value) -> Result<bool, ToolError> {
        Ok(self.plan(args).await?.indexed_root.is_some())
    }

    async fn preview_in(
        &self,
        args: &Value,
        ctx: &ToolContext,
    ) -> Result<ApprovalPreview, ToolError> {
        let tool = app_tools::EXPORT_DOCUMENT;
        let markdown =
            str_arg(args, "markdown").ok_or_else(|| invalid(tool, "`markdown` is required"))?;
        let sources = resolve_sources(tool, markdown, args, ctx)?;
        let plan = self.plan(args).await?;
        let file = plan
            .path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.approved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(ctx.step_id.clone(), plan.path.clone());
        let label = match &plan.indexed_root {
            Some(root) => format!("Save {file} inside the indexed folder {}", root.display()),
            None => format!("Save {file} to {}", plan.folder.display()),
        };
        Ok(ApprovalPreview {
            label: Some(label),
            details: json!({
                "file": file,
                "path": plan.path.display().to_string(),
                "format": plan.format.extension(),
                "sources": sources.len(),
                "insideIndexedFolder": plan.indexed_root.map(|r| r.display().to_string()),
                "createsFolder": plan.create_folder,
            }),
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::EXPORT_DOCUMENT;
        let markdown =
            str_arg(&args, "markdown").ok_or_else(|| invalid(tool, "`markdown` is required"))?;
        let sources = resolve_sources(tool, markdown, &args, ctx)?;
        let plan = self.plan(&args).await?;
        let approved = self
            .approved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&ctx.step_id);
        if plan.indexed_root.is_some() && approved.is_none() {
            return Err(ToolError::Forbidden(
                "Saving inside an indexed folder needs the user's approval of the exact path."
                    .to_string(),
            ));
        }
        let bytes = match plan.format {
            Format::Markdown => markdown_bytes(&plan.title, markdown, &sources),
            Format::Docx => docx_bytes(&plan.title, markdown, &sources)?,
        };
        if plan.create_folder {
            std::fs::create_dir_all(&plan.folder).map_err(|e| {
                ToolError::Unavailable(format!("Could not create {}: {e}", plan.folder.display()))
            })?;
        }
        let (path, mut file) = match approved {
            Some(path) => {
                // The approval named this exact file; anything else (a
                // different folder now, or a file that appeared since) is
                // refused rather than silently redirected.
                let same_folder = path
                    .parent()
                    .is_some_and(|p| comparable(p) == comparable(&plan.folder));
                if !same_folder {
                    return Err(ToolError::Failed(
                        "The destination changed after approval; ask again.".to_string(),
                    ));
                }
                let file = create_exact(&path).map_err(|e| {
                    ToolError::Failed(format!(
                        "{} could not be created ({e}); it may have appeared after approval. Ask again.",
                        path.display()
                    ))
                })?;
                (path, file)
            }
            None => {
                let stem = plan
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Shodh export".to_string());
                create_unique(&plan.folder, &stem, plan.format.extension()).map_err(|e| {
                    ToolError::Unavailable(format!("Could not save the export: {e}"))
                })?
            }
        };
        if let Err(e) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            drop(file);
            if let Err(cleanup) = std::fs::remove_file(&path) {
                tracing::warn!(path = %path.display(), error = %cleanup, "removing a partial export failed");
            }
            return Err(ToolError::Failed(format!(
                "Writing {} failed: {e}",
                path.display()
            )));
        }
        let shown = path.display().to_string();
        Ok(ToolOutput {
            text_for_model: format!(
                "Saved {shown} ({} KB, {} sources).",
                bytes.len().div_ceil(1024),
                sources.len()
            ),
            summary_for_ui: format!(
                "Saved {}",
                path.file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| shown.clone())
            ),
            detail: Some(json!({
                "path": shown,
                "folder": plan.folder.display().to_string(),
                "format": plan.format.extension(),
                "bytes": bytes.len(),
                "sources": sources.iter().map(SourceEntry::line).collect::<Vec<_>>(),
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use std::io::Read;
    use std::path::Path;

    fn passage(n: u32, file: &str, path: &str, page: Option<&str>, web: bool) -> CitedPassage {
        CitedPassage {
            n,
            file: file.into(),
            path: path.into(),
            page: page.map(str::to_string),
            web,
            text: String::new(),
            checkable: true,
        }
    }

    fn ctx_with_passages() -> ToolContext {
        let (ctx, _rx) = testing::ctx();
        ctx.reserve_passages(2);
        ctx.record_passage(passage(
            1,
            "lease.pdf",
            "c:/docs/lease.pdf",
            Some("4"),
            false,
        ));
        ctx.record_passage(passage(
            2,
            "Rust blog",
            "https://blog.example/r",
            None,
            true,
        ));
        ctx
    }

    const BODY: &str = "## Findings\n\nThe notice period is **60 days** [1]. Rust shipped async closures [2].\n\n| Term | Days |\n|---|---|\n| Notice | 60 |\n\n```\nnot a citation [9]\n```\n";

    #[test]
    fn citations_outside_code_are_found() {
        assert_eq!(cited_numbers(BODY), vec![1, 2]);
    }

    #[test]
    fn unknown_citations_fail_unless_described() {
        let ctx = ctx_with_passages();
        let tool = app_tools::EXPORT_DOCUMENT;
        let err = resolve_sources(tool, "Claim [3].", &json!({}), &ctx).unwrap_err();
        assert!(err.to_string().contains("[3]"));
        let ok = resolve_sources(
            tool,
            "Claim [3]. Other [1].",
            &json!({"sources": [{"n": 3, "title": "memo.docx", "location": "page 2"}]}),
            &ctx,
        )
        .unwrap();
        assert_eq!(ok[0].line(), "[1] lease.pdf, page 4");
        assert_eq!(ok[1].line(), "[3] memo.docx, page 2");
    }

    #[test]
    fn markdown_export_appends_sources() {
        let ctx = ctx_with_passages();
        let sources = resolve_sources(app_tools::EXPORT_DOCUMENT, BODY, &json!({}), &ctx).unwrap();
        let text = String::from_utf8(markdown_bytes("Lease notes", BODY, &sources)).unwrap();
        assert!(text.starts_with("# Lease notes\n\n## Findings"));
        assert!(text.contains(
            "## Sources\n\n- [1] lease.pdf, page 4\n- [2] Rust blog — https://blog.example/r\n"
        ));
    }

    #[test]
    fn docx_export_is_a_word_package_with_the_text_and_sources() {
        let ctx = ctx_with_passages();
        let sources = resolve_sources(app_tools::EXPORT_DOCUMENT, BODY, &json!({}), &ctx).unwrap();
        let bytes = docx_bytes("Lease notes", BODY, &sources).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("60 days"));
        assert!(xml.contains("Heading2"));
        assert!(xml.contains("[1] lease.pdf"));
        assert!(xml.contains("w:tbl"), "markdown tables become Word tables");
        assert!(xml.contains("not a citation [9]"));
    }

    #[tokio::test]
    async fn exports_go_to_documents_and_never_overwrite() {
        let t = testing::host().await;
        let tool = ExportDocumentTool {
            host: t.host.clone(),
            approved: Mutex::new(HashMap::new()),
        };
        let ctx = ctx_with_passages();
        let args = json!({"title": "Lease: notes?", "markdown": BODY, "format": "md"});
        assert!(!tool.must_confirm(&args).await.unwrap());
        let first = tool.execute(args.clone(), &ctx).await.unwrap();
        let second = tool.execute(args, &ctx).await.unwrap();
        let p1 = first.detail.unwrap()["path"].as_str().unwrap().to_string();
        let p2 = second.detail.unwrap()["path"].as_str().unwrap().to_string();
        assert!(p1.ends_with("Lease_ notes_.md"), "{p1}");
        assert!(p2.ends_with("Lease_ notes_ (2).md"), "{p2}");
        assert!(Path::new(&p1).starts_with(t.effects.documents.join(EXPORTS_FOLDER)));
    }

    #[tokio::test]
    async fn exports_into_indexed_folders_need_approval_of_that_exact_path() {
        let t = testing::host().await;
        let tool = ExportDocumentTool {
            host: t.host.clone(),
            approved: Mutex::new(HashMap::new()),
        };
        let indexed = t.dir.path().join("indexed");
        std::fs::create_dir_all(&indexed).unwrap();
        testing::index_folder(&t, &indexed).await;
        let ctx = ctx_with_passages();
        let args = json!({
            "title": "Summary",
            "markdown": "Text [1].",
            "folder": indexed.display().to_string(),
        });
        assert!(tool.must_confirm(&args).await.unwrap());
        // Without a preview (no approval) the write is refused.
        assert!(matches!(
            tool.execute(args.clone(), &ctx).await,
            Err(ToolError::Forbidden(_))
        ));
        let preview = tool.preview_in(&args, &ctx).await.unwrap();
        assert!(preview.label.unwrap().contains("inside the indexed folder"));
        let approved_path = preview.details["path"].as_str().unwrap().to_string();
        // A file appears at the approved path before execution: refused,
        // not redirected to a new name.
        std::fs::write(&approved_path, b"user file").unwrap();
        let err = tool.execute(args.clone(), &ctx).await.unwrap_err();
        assert!(err.to_string().contains("ask again") || err.to_string().contains("Ask again"));
        assert_eq!(std::fs::read(&approved_path).unwrap(), b"user file");
        std::fs::remove_file(&approved_path).unwrap();
        tool.preview_in(&args, &ctx).await.unwrap();
        let out = tool.execute(args, &ctx).await.unwrap();
        assert_eq!(out.detail.unwrap()["path"], approved_path);
    }
}
