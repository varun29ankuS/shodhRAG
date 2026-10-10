//! Citation graph tools (all read): `paper_graph` (who cites whom among the user's papers,
//! what the library builds on, shared references, a chain of citations between two
//! papers), `get_paper` (one paper: metadata, library status, results, snippets) and
//! `find_papers` (papers by method, dataset, author or year).
//!
//! Who-cites-whom is cited with the reference entry as printed in the citing library
//! paper (file, page, box), so the answer check verifies "A cites B" against the
//! bibliography text. Facts that come from the graph's own records (counts, methods,
//! library status) are numbered as passages that only have their numbers checked.
//! Building the graph (which may contact OpenAlex) is the user's, in Library → Graph.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::{
    CitedPassage, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
    UNTRUSTED_NOTICE,
};
use shodh_rag::harness::RiskTier;
use shodh_rag::research::citations::graph::{CitationEvidence, PaperGraph, PaperHit, PaperNode};
use shodh_rag::research::citations::views::{find, lineage, paper_view};
use shodh_rag::research::citations::PaperFilter;
use shodh_rag::research::file_name;

use super::{invalid, limit_arg, str_arg, AgentHost};
use crate::research_commands::ResearchCommandError;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;
/// Characters of a reference entry shown per passage.
const EVIDENCE_CHARS: usize = 600;

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(PaperGraphTool { host: host.clone() }))?;
    registry.register(Arc::new(GetPaperTool { host: host.clone() }))?;
    registry.register(Arc::new(FindPapersTool { host: host.clone() }))?;
    Ok(())
}

fn tool_error(error: ResearchCommandError) -> ToolError {
    match error.code {
        "not_found" => ToolError::NotFound(error.message),
        _ => ToolError::Failed(error.message),
    }
}

async fn graph_of(host: &AgentHost) -> Result<Arc<PaperGraph>, ToolError> {
    let services = host.research.services().await.map_err(tool_error)?;
    services
        .citations
        .graph(&services.results)
        .await
        .map_err(|e| tool_error(e.into()))
}

const NOT_BUILT: &str = "The paper graph has not been built yet. The user can build it in Library → Graph (Build); until then, answer from search_documents.";

fn find_paper<'g>(
    graph: &'g PaperGraph,
    tool: &str,
    query: &str,
) -> Result<&'g PaperNode, ToolError> {
    if graph.papers().is_empty() {
        return Err(ToolError::Failed(NOT_BUILT.to_string()));
    }
    graph.find(query).ok_or_else(|| {
        ToolError::NotFound(format!(
            "{tool}: no paper in the graph matches \"{query}\" (try its title, arXiv id, DOI or file path)"
        ))
    })
}

fn describe(paper: &PaperNode) -> String {
    let mut text = paper.label();
    if let Some(y) = paper.year {
        text.push_str(&format!(" ({y})"));
    }
    if let Some(a) = paper.authors.as_deref() {
        let short: String = a.chars().take(120).collect();
        text.push_str(&format!(", {short}"));
    }
    if paper.in_library {
        text.push_str(" — in the user's library");
        if let Some(f) = &paper.file_path {
            text.push_str(&format!(" ({})", file_name(f)));
        }
    }
    text
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Whether `paper` may appear in this answer: always, unless the answer is limited to a
/// workspace, where library papers outside its sources are left out (works outside the
/// library are public bibliographic records and stay).
fn shown(ctx: &ToolContext, paper: &PaperNode) -> bool {
    !ctx.scope().restricted
        || !paper.in_library
        || paper
            .file_path
            .as_deref()
            .is_some_and(|f| ctx.scope().allows_path(f))
}

/// Refuses a paper the answer may not use.
fn require_shown(ctx: &ToolContext, paper: &PaperNode) -> Result<(), ToolError> {
    if shown(ctx, paper) {
        Ok(())
    } else {
        Err(ToolError::Forbidden(ctx.scope().outside_message(&format!(
            "The paper \"{}\"",
            paper.label()
        ))))
    }
}

/// Numbers a reference entry as a checkable passage of the citing library paper.
fn cite_evidence(
    ctx: &ToolContext,
    citing: &PaperNode,
    evidence: &CitationEvidence,
    passages: &mut Vec<Value>,
) -> Option<u32> {
    if !shown(ctx, citing) {
        return None;
    }
    let path = citing.file_path.clone()?;
    let n = ctx.reserve_passages(1);
    let text = clip(&evidence.text, EVIDENCE_CHARS);
    ctx.record_passage(CitedPassage {
        n,
        file: file_name(&path),
        path: path.clone(),
        page: evidence.page.map(|p| p.to_string()),
        web: false,
        text: text.clone(),
        checkable: true,
    });
    let mut passage = json!({
        "n": n,
        "file": file_name(&path),
        "path": path,
        "page": evidence.page.map(|p| p.to_string()),
        "score": 1.0,
        "text": text,
    });
    if !evidence.regions.is_empty() {
        if let Some(map) = passage.as_object_mut() {
            map.insert(
                "regions".to_string(),
                json!(evidence
                    .regions
                    .iter()
                    .map(
                        |r| json!({"page": r.page, "x0": r.x0, "y0": r.y0, "x1": r.x1, "y1": r.y1})
                    )
                    .collect::<Vec<_>>()),
            );
        }
    }
    passages.push(passage);
    Some(n)
}

/// Numbers a graph record (not checkable text) as a passage.
fn cite_record(
    ctx: &ToolContext,
    paper: &PaperNode,
    text: String,
    passages: &mut Vec<Value>,
) -> u32 {
    let n = ctx.reserve_passages(1);
    let (file, path) = match &paper.file_path {
        Some(p) => (file_name(p), p.clone()),
        None => (paper.label(), format!("graph://{}", paper.id)),
    };
    ctx.record_passage(CitedPassage {
        n,
        file: file.clone(),
        path: path.clone(),
        page: None,
        web: false,
        text: text.clone(),
        checkable: false,
    });
    passages.push(json!({ "n": n, "file": file, "path": path, "score": 1.0, "text": text }));
    n
}

pub struct PaperGraphTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for PaperGraphTool {
    fn name(&self) -> &'static str {
        app_tools::PAPER_GRAPH
    }
    fn label(&self) -> &'static str {
        "Paper graph"
    }
    fn label_template(&self) -> &'static str {
        "Reading the paper graph[ ({op})][ for {paper}]"
    }
    fn description(&self) -> &'static str {
        "Read the citation graph of the user's papers (built from their bibliographies). \
         `op`: `cited` (what `paper` cites, split into works in the library and elsewhere), \
         `citers` (library papers citing `paper`), `neighbors` (both, plus related library \
         papers sharing references), `shared_references` (works both `paper` and `other` cite), \
         `lineage` (a shortest chain of citations between `paper` and `other`), `most_cited` \
         (the works the most library papers cite: what the library builds on; no `paper` \
         needed). Papers are named by title, arXiv id, DOI or file path. Each citation comes \
         with the reference entry as printed, numbered n: cite who-cites-whom with it."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["cited", "citers", "neighbors", "shared_references", "lineage", "most_cited"]},
                "paper": {"type": "string", "minLength": 1, "maxLength": 1000},
                "other": {"type": "string", "minLength": 1, "maxLength": 1000},
                "in_library_only": {"type": "boolean"},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT}
            },
            "required": ["op"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = self.name();
        let op = str_arg(&args, "op").ok_or_else(|| invalid(tool, "`op` is required"))?;
        let limit = limit_arg(&args, DEFAULT_LIMIT, MAX_LIMIT);
        let library_only = args
            .get("in_library_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let graph = graph_of(&self.host).await?;
        if graph.papers().is_empty() {
            return Err(ToolError::Failed(NOT_BUILT.to_string()));
        }
        let mut lines: Vec<String> = Vec::new();
        let mut passages: Vec<Value> = Vec::new();
        let summary;
        match op {
            "most_cited" => {
                let hits: Vec<PaperHit> = graph.most_cited(limit, 2);
                if hits.is_empty() {
                    lines.push("No work is cited by two or more library papers.".to_string());
                }
                for hit in &hits {
                    let citers: Vec<&PaperNode> = graph
                        .citers(&hit.paper.id)
                        .into_iter()
                        .filter(|c| shown(ctx, c))
                        .collect();
                    if citers.is_empty() {
                        continue;
                    }
                    let first = citers.iter().find_map(|c| {
                        graph
                            .evidence(&c.id, &hit.paper.id)
                            .map(|e| (*c, e.clone()))
                    });
                    let n = first.and_then(|(c, e)| cite_evidence(ctx, c, &e, &mut passages));
                    let names: Vec<String> = citers
                        .iter()
                        .filter_map(|c| c.file_path.as_deref().map(file_name))
                        .collect();
                    lines.push(format!(
                        "{}{} — cited by {} library papers: {}",
                        n.map(|n| format!("[{n}] ")).unwrap_or_default(),
                        describe(&hit.paper),
                        citers.len(),
                        names.join(", ")
                    ));
                }
                summary = format!("{} works the library builds on", hits.len());
            }
            "cited" | "citers" | "neighbors" => {
                let paper = find_paper(
                    &graph,
                    tool,
                    str_arg(&args, "paper").ok_or_else(|| invalid(tool, "`paper` is required"))?,
                )?;
                require_shown(ctx, paper)?;
                lines.push(format!("Paper: {}", describe(paper)));
                if op != "citers" {
                    let cited: Vec<&PaperNode> = graph
                        .cited(&paper.id)
                        .into_iter()
                        .filter(|p| !library_only || p.in_library)
                        .filter(|p| shown(ctx, p))
                        .collect();
                    lines.push(format!(
                        "Cites {} works ({} in the library):",
                        cited.len(),
                        cited.iter().filter(|p| p.in_library).count()
                    ));
                    let mut ordered = cited.clone();
                    ordered.sort_by_key(|p| (!p.in_library, std::cmp::Reverse(p.cited_by_count)));
                    for c in ordered.into_iter().take(limit) {
                        let n = graph
                            .evidence(&paper.id, &c.id)
                            .and_then(|e| cite_evidence(ctx, paper, e, &mut passages));
                        lines.push(format!(
                            "{}{}",
                            n.map(|n| format!("[{n}] ")).unwrap_or_default(),
                            describe(c)
                        ));
                    }
                }
                if op != "cited" {
                    let citers: Vec<&PaperNode> = graph
                        .citers(&paper.id)
                        .into_iter()
                        .filter(|c| shown(ctx, c))
                        .collect();
                    lines.push(format!("Cited by {} library papers:", citers.len()));
                    for c in citers.into_iter().take(limit) {
                        let n = graph
                            .evidence(&c.id, &paper.id)
                            .and_then(|e| cite_evidence(ctx, c, e, &mut passages));
                        lines.push(format!(
                            "{}{}",
                            n.map(|n| format!("[{n}] ")).unwrap_or_default(),
                            describe(c)
                        ));
                    }
                }
                if op == "neighbors" {
                    let mut related = graph.related(&paper.id, limit.min(10));
                    related.retain(|r| shown(ctx, &r.paper));
                    if !related.is_empty() {
                        lines.push("Library papers sharing references:".to_string());
                        for r in related {
                            let n = cite_record(
                                ctx,
                                &r.paper,
                                format!(
                                    "{} shares {} references with {}",
                                    describe(&r.paper),
                                    r.count,
                                    paper.label()
                                ),
                                &mut passages,
                            );
                            lines.push(format!(
                                "[{n}] {} — {} shared references",
                                describe(&r.paper),
                                r.count
                            ));
                        }
                    }
                }
                summary = format!("Citations of {}", clip(&paper.label(), 60));
            }
            "shared_references" | "lineage" => {
                let a = find_paper(
                    &graph,
                    tool,
                    str_arg(&args, "paper").ok_or_else(|| invalid(tool, "`paper` is required"))?,
                )?;
                let b = find_paper(
                    &graph,
                    tool,
                    str_arg(&args, "other").ok_or_else(|| invalid(tool, "`other` is required"))?,
                )?;
                require_shown(ctx, a)?;
                require_shown(ctx, b)?;
                if op == "shared_references" {
                    let mut shared = graph.shared_references(&a.id, &b.id);
                    shared.retain(|s| shown(ctx, s));
                    lines.push(format!(
                        "{} and {} both cite {} works:",
                        a.label(),
                        b.label(),
                        shared.len()
                    ));
                    for s in shared.into_iter().take(limit) {
                        let na = graph
                            .evidence(&a.id, &s.id)
                            .and_then(|e| cite_evidence(ctx, a, e, &mut passages));
                        let nb = graph
                            .evidence(&b.id, &s.id)
                            .and_then(|e| cite_evidence(ctx, b, e, &mut passages));
                        let marks: String = [na, nb]
                            .iter()
                            .flatten()
                            .map(|n| format!("[{n}]"))
                            .collect();
                        lines.push(format!("{marks} {}", describe(s)));
                    }
                    summary = "Shared references".to_string();
                } else {
                    // A chain through a library paper outside the workspace is not shown.
                    let chain = lineage(&graph, &a.id, &b.id)
                        .filter(|path| path.iter().all(|step| shown(ctx, &step.paper)));
                    match chain {
                        Some(path) => {
                            lines.push(format!("A chain of {} citations:", path.len().saturating_sub(1)));
                            for (k, step) in path.iter().enumerate() {
                                if k == 0 {
                                    lines.push(describe(&step.paper));
                                    continue;
                                }
                                let prev = &path[k - 1].paper;
                                let (citing, cited) = if step.relation == Some("cites") {
                                    (prev, &step.paper)
                                } else {
                                    (&step.paper, prev)
                                };
                                let n = graph
                                    .evidence(&citing.id, &cited.id)
                                    .and_then(|e| cite_evidence(ctx, citing, e, &mut passages));
                                let verb = if step.relation == Some("cites") { "cites" } else { "is cited by" };
                                lines.push(format!(
                                    "→ {verb} {}{}",
                                    describe(&step.paper),
                                    n.map(|n| format!(" [{n}]")).unwrap_or_default()
                                ));
                            }
                        }
                        None => lines.push(format!(
                            "No chain of citations within six steps connects {} and {} in the graph.",
                            a.label(),
                            b.label()
                        )),
                    }
                    summary = "Citation lineage".to_string();
                }
            }
            other => return Err(invalid(tool, format!("unknown op `{other}`"))),
        }
        Ok(ToolOutput {
            text_for_model: format!("{UNTRUSTED_NOTICE}\n{}", lines.join("\n")),
            summary_for_ui: summary,
            detail: Some(json!({ "passages": passages })),
        })
    }
}

pub struct GetPaperTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for GetPaperTool {
    fn name(&self) -> &'static str {
        app_tools::GET_PAPER
    }
    fn label(&self) -> &'static str {
        "Open paper record"
    }
    fn label_template(&self) -> &'static str {
        "Reading the record of {paper}"
    }
    fn description(&self) -> &'static str {
        "One paper from the citation graph, named by title, arXiv id, DOI or file path: title, \
         authors, venue and year, DOI/arXiv/OpenAlex links, whether it is in the user's library, \
         how many works it cites (in the library and elsewhere), which library papers cite it, \
         the methods and datasets of its results, the results read from its tables (each \
         numbered n with its cell) and the user's snippets of it."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "paper": {"type": "string", "minLength": 1, "maxLength": 1000} },
            "required": ["paper"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = self.name();
        let query = str_arg(&args, "paper").ok_or_else(|| invalid(tool, "`paper` is required"))?;
        let graph = graph_of(&self.host).await?;
        let paper = find_paper(&graph, tool, query)?.clone();
        require_shown(ctx, &paper)?;
        let view = paper_view(&graph, &paper.id).ok_or_else(|| {
            ToolError::NotFound(format!("{tool}: \"{query}\" is not in the graph"))
        })?;
        let mut passages = Vec::new();
        let mut lines = Vec::new();
        let mut record = describe(&paper);
        if let Some(v) = &paper.venue {
            record.push_str(&format!("; venue: {v}"));
        }
        if let Some(c) = paper.cited_by_count {
            record.push_str(&format!("; cited by {c} works according to OpenAlex"));
        }
        let n = cite_record(ctx, &paper, record.clone(), &mut passages);
        lines.push(format!("[{n}] {record}"));
        for (label, link) in [
            ("DOI", &view.links.doi),
            ("arXiv", &view.links.arxiv),
            ("OpenAlex", &view.links.openalex),
        ] {
            if let Some(l) = link {
                lines.push(format!("{label}: {l}"));
            }
        }
        lines.push(format!(
            "Cites {} works in the library and {} elsewhere; cited by {} library papers.",
            view.cites_in_library.len(),
            view.cites_elsewhere.len(),
            view.cited_by_in_library.len()
        ));
        for linked in view
            .cited_by_in_library
            .iter()
            .filter(|l| shown(ctx, &l.paper))
            .take(10)
        {
            let n = linked
                .evidence
                .as_ref()
                .and_then(|e| cite_evidence(ctx, &linked.paper, e, &mut passages));
            lines.push(format!(
                "{}Cited by {}",
                n.map(|n| format!("[{n}] ")).unwrap_or_default(),
                describe(&linked.paper)
            ));
        }
        if !view.methods.is_empty() {
            lines.push(format!(
                "Methods: {}",
                view.methods
                    .iter()
                    .map(|m| m.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !view.datasets.is_empty() {
            lines.push(format!(
                "Datasets: {}",
                view.datasets
                    .iter()
                    .map(|d| d.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !view.proposes.is_empty() {
            lines.push(format!(
                "Proposes (from its title or abstract): {}",
                view.proposes
                    .iter()
                    .map(|m| m.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(file) = &paper.file_path {
            let services = self.host.research.services().await.map_err(tool_error)?;
            let results = services
                .results
                .list(file)
                .await
                .map_err(|e| tool_error(e.into()))?
                .results;
            if !results.is_empty() {
                lines.push("Results (each from one table cell):".to_string());
            }
            for r in results.iter().take(40) {
                let n = ctx.reserve_passages(1);
                let value = format!(
                    "{} — {} on {}: {}",
                    r.method, r.metric, r.dataset, r.value_text
                );
                ctx.record_passage(CitedPassage {
                    n,
                    file: r.file_name.clone(),
                    path: r.file_path.clone(),
                    page: Some(r.page.to_string()),
                    web: false,
                    text: value.clone(),
                    checkable: true,
                });
                let mut passage = json!({"n": n, "file": r.file_name, "path": r.file_path, "page": r.page.to_string(), "score": 1.0, "text": value});
                if let (Some(region), Some(map)) = (r.region, passage.as_object_mut()) {
                    map.insert(
                        "regions".to_string(),
                        json!([{ "page": region.page, "x0": region.x0, "y0": region.y0, "x1": region.x1, "y1": region.y1 }]),
                    );
                }
                passages.push(passage);
                lines.push(format!("[{n}] {value}, page {}", r.page));
            }
            if results.len() > 40 {
                lines.push(format!(
                    "- {} more results; use query_results to filter.",
                    results.len() - 40
                ));
            }
            let snippets = services
                .snippets
                .list(&shodh_rag::research::snippets::SnippetQuery {
                    file_path: Some(file.clone()),
                    text: None,
                    scopes: super::papers::run_scopes(ctx),
                    limit: Some(10),
                })
                .await
                .map_err(|e| tool_error(e.into()))?;
            for s in &snippets {
                let n = ctx.reserve_passages(1);
                let text = clip(&s.text, 400);
                ctx.record_passage(CitedPassage {
                    n,
                    file: s.file_name.clone(),
                    path: s.file_path.clone(),
                    page: Some(s.page.to_string()),
                    web: false,
                    text: text.clone(),
                    checkable: true,
                });
                passages.push(json!({"n": n, "file": s.file_name, "path": s.file_path, "page": s.page.to_string(), "score": 1.0, "text": text}));
                lines.push(format!(
                    "[{n}] Snippet{}: {text}",
                    Some(s.title.as_str())
                        .filter(|t| !t.trim().is_empty())
                        .map(|t| format!(" \"{t}\""))
                        .unwrap_or_default()
                ));
            }
        }
        Ok(ToolOutput {
            text_for_model: format!("{UNTRUSTED_NOTICE}\n{}", lines.join("\n")),
            summary_for_ui: clip(&paper.label(), 80),
            detail: Some(json!({ "passages": passages, "paperId": paper.id })),
        })
    }
}

pub struct FindPapersTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for FindPapersTool {
    fn name(&self) -> &'static str {
        app_tools::FIND_PAPERS
    }
    fn label(&self) -> &'static str {
        "Find papers"
    }
    fn label_template(&self) -> &'static str {
        "Finding papers[ using {method}][ on {dataset}][ by {author}]"
    }
    fn description(&self) -> &'static str {
        "List papers in the citation graph matching every given filter: `method` or `dataset` \
         (names as printed in results tables, e.g. \"DeltaNet\", \"WikiText-103\"; methods also \
         match the library paper that proposed them), `author` (surname or full name), \
         `year_from`/`year_to`, `in_library_only`. Library papers come first, newest first."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": {"type": "string", "minLength": 1, "maxLength": 200},
                "dataset": {"type": "string", "minLength": 1, "maxLength": 200},
                "author": {"type": "string", "minLength": 1, "maxLength": 200},
                "year_from": {"type": "integer", "minimum": 1900, "maximum": 2100},
                "year_to": {"type": "integer", "minimum": 1900, "maximum": 2100},
                "in_library_only": {"type": "boolean"},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = self.name();
        let year = |key: &str| {
            args.get(key)
                .and_then(Value::as_i64)
                .and_then(|y| i32::try_from(y).ok())
        };
        let filter = PaperFilter {
            method: str_arg(&args, "method").map(str::to_string),
            dataset: str_arg(&args, "dataset").map(str::to_string),
            author: str_arg(&args, "author").map(str::to_string),
            year_from: year("year_from"),
            year_to: year("year_to"),
            in_library_only: args
                .get("in_library_only")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        if filter.method.is_none()
            && filter.dataset.is_none()
            && filter.author.is_none()
            && filter.year_from.is_none()
            && filter.year_to.is_none()
            && !filter.in_library_only
        {
            return Err(invalid(tool, "give at least one filter"));
        }
        let graph = graph_of(&self.host).await?;
        if graph.papers().is_empty() {
            return Err(ToolError::Failed(NOT_BUILT.to_string()));
        }
        let limit = limit_arg(&args, DEFAULT_LIMIT, MAX_LIMIT);
        let mut found = find(&graph, &filter, limit);
        found.retain(|p| shown(ctx, p));
        let mut passages = Vec::new();
        let mut lines = Vec::new();
        if found.is_empty() {
            lines.push("No paper in the graph matches.".to_string());
        }
        for paper in &found {
            let text = describe(paper);
            let n = cite_record(ctx, paper, text.clone(), &mut passages);
            lines.push(format!("[{n}] {text}"));
        }
        Ok(ToolOutput {
            text_for_model: format!("{UNTRUSTED_NOTICE}\n{}", lines.join("\n")),
            summary_for_ui: format!(
                "{} {}",
                found.len(),
                if found.len() == 1 { "paper" } else { "papers" }
            ),
            detail: Some(json!({ "passages": passages })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_tools::testing;

    #[tokio::test]
    async fn graph_tools_say_when_the_graph_is_not_built() {
        let t = testing::host().await;
        let tool = PaperGraphTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let err = tool
            .execute(json!({ "op": "most_cited" }), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Failed(m) if m.contains("Library → Graph")));
        let find = FindPapersTool {
            host: t.host.clone(),
        };
        let err = find.execute(json!({}), &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
        assert_eq!(tool.tier(), RiskTier::Read);
        assert_eq!(find.tier(), RiskTier::Read);
        assert_eq!(
            GetPaperTool {
                host: t.host.clone()
            }
            .tier(),
            RiskTier::Read
        );
    }

    fn paper(id: &str, title: &str, year: i32, file: Option<&str>) -> PaperNode {
        PaperNode {
            id: id.into(),
            title: Some(title.into()),
            year: Some(year),
            doi: None,
            arxiv_id: id.strip_prefix("paper:arxiv:").map(str::to_string),
            openalex_id: None,
            authors: Some("Songlin Yang, Yoon Kim".into()),
            author_ids: vec!["author:yang-s".into(), "author:kim-y".into()],
            venue: None,
            in_library: file.is_some(),
            file_path: file.map(str::to_string),
            cited_by_count: None,
            statement_id: None,
        }
    }

    async fn with_graph(t: &testing::TestHost) {
        use shodh_rag::research::citations::graph::{ConceptNode, GraphParts};
        let evidence = |text: &str| CitationEvidence {
            text: text.into(),
            page: Some(11),
            regions: vec![],
        };
        let parts = GraphParts {
            papers: vec![
                paper("paper:arxiv:2406.06484", "Parallelizing Linear Transformers with the Delta Rule over Sequence Length", 2024, Some("C:/p/delta.pdf")),
                paper("paper:arxiv:2411.12537", "Unlocking State-Tracking in Linear RNNs Through Negative Eigenvalues", 2024, Some("C:/p/neg.pdf")),
                paper("paper:arxiv:2102.11174", "Linear Transformers Are Secretly Fast Weight Programmers", 2021, Some("C:/p/fwp.pdf")),
                paper("paper:arxiv:2006.16236", "Transformers are RNNs", 2020, None),
            ],
            cites: vec![
                ("paper:arxiv:2411.12537".into(), "paper:arxiv:2406.06484".into(), Some(evidence("S. Yang, B. Wang, Y. Zhang, Y. Shen, and Y. Kim. Parallelizing linear transformers with the delta rule over sequence length. In NeurIPS, 2024."))),
                ("paper:arxiv:2411.12537".into(), "paper:arxiv:2006.16236".into(), Some(evidence("A. Katharopoulos et al. Transformers are RNNs. In ICML, 2020."))),
                ("paper:arxiv:2406.06484".into(), "paper:arxiv:2006.16236".into(), Some(evidence("Angelos Katharopoulos et al. 2020. Transformers are RNNs. In ICML."))),
                ("paper:arxiv:2406.06484".into(), "paper:arxiv:2102.11174".into(), Some(evidence("Imanol Schlag, Kazuki Irie, and Jürgen Schmidhuber. 2021. Linear transformers are secretly fast weight programmers. In ICML."))),
            ],
            methods: vec![ConceptNode { id: "method:deltanet".into(), label: "DeltaNet".into() }],
            uses_method: vec![("paper:arxiv:2411.12537".into(), "method:deltanet".into())],
            proposed_in: vec![("method:deltanet".into(), "paper:arxiv:2406.06484".into())],
            ..GraphParts::default()
        };
        let services = t.host.research.services().await.unwrap();
        *services.citations.slot().write() = Some(Arc::new(PaperGraph::new(parts)));
    }

    #[tokio::test]
    async fn paper_graph_cites_who_cites_whom_with_the_reference_entry() {
        let t = testing::host().await;
        with_graph(&t).await;
        let tool = PaperGraphTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let out = tool
            .execute(json!({ "op": "citers", "paper": "2406.06484" }), &ctx)
            .await
            .unwrap();
        assert!(
            out.text_for_model.contains("Cited by 1 library papers"),
            "{}",
            out.text_for_model
        );
        assert!(out.text_for_model.contains("[1] Unlocking State-Tracking"));
        let passage = ctx.cited_passage(1).unwrap();
        assert!(passage
            .text
            .contains("Parallelizing linear transformers with the delta rule"));
        assert_eq!(passage.path, "C:/p/neg.pdf");
        assert!(passage.checkable);

        let (ctx, _rx) = testing::ctx();
        let out = tool
            .execute(json!({ "op": "most_cited" }), &ctx)
            .await
            .unwrap();
        assert!(out.text_for_model.contains("Transformers are RNNs (2020)"));
        assert!(out.text_for_model.contains("cited by 2 library papers"));

        let (ctx, _rx) = testing::ctx();
        let out = tool
            .execute(json!({ "op": "lineage", "paper": "2411.12537", "other": "Linear Transformers Are Secretly Fast Weight Programmers" }), &ctx)
            .await
            .unwrap();
        assert!(
            out.text_for_model.contains("A chain of 2 citations"),
            "{}",
            out.text_for_model
        );

        let (ctx, _rx) = testing::ctx();
        let out = tool
            .execute(
                json!({ "op": "shared_references", "paper": "2411.12537", "other": "2406.06484" }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.text_for_model.contains("both cite 1 works"));
        assert_eq!(ctx.passages_issued(), 2);

        let find = FindPapersTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let out = find
            .execute(json!({ "method": "DeltaNet" }), &ctx)
            .await
            .unwrap();
        assert_eq!(out.summary_for_ui, "2 papers");
        let (ctx, _rx) = testing::ctx();
        let out = find
            .execute(json!({ "author": "Kim", "year_to": 2020 }), &ctx)
            .await
            .unwrap();
        assert_eq!(out.summary_for_ui, "1 paper");

        let get = GetPaperTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let out = get
            .execute(json!({ "paper": "2406.06484" }), &ctx)
            .await
            .unwrap();
        assert!(out.text_for_model.contains("in the user's library"));
        assert!(out
            .text_for_model
            .contains("Proposes (from its title or abstract): DeltaNet"));
        assert!(out.text_for_model.contains("cited by 1 library papers"));
        let err = get
            .execute(json!({ "paper": "no such paper" }), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)));
    }
}
