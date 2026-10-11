//! What the paper page, the method and dataset pages, the graph view and the agent tools
//! read from the graph, as serialisable values. Results and snippets of a paper are added
//! by the app (they live in the statement store under the paper's file).

use serde::Serialize;

use super::graph::{
    author_surname, CitationEvidence, ConceptNode, PaperFilter, PaperGraph, PaperHit, PaperNode,
    PathStep,
};

/// Most works listed per section of a paper page.
pub const MAX_LISTED: usize = 500;
/// Most related papers on a paper page.
pub const MAX_RELATED: usize = 12;
/// Most nodes sent to the graph view (it clusters what it does not draw).
pub const MAX_VIEW_NODES: usize = 3_000;

/// A cited or citing work with where the citation is printed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedPaper {
    pub paper: PaperNode,
    /// The reference entry in the citing library paper, when known.
    pub evidence: Option<CitationEvidence>,
    /// The library file that prints the reference.
    pub citing_file: Option<String>,
}

/// An author of a paper.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorRef {
    pub id: String,
    pub name: String,
}

/// Links to a paper elsewhere.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperLinks {
    pub doi: Option<String>,
    pub arxiv: Option<String>,
    pub openalex: Option<String>,
}

/// Everything the graph says about one paper.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperView {
    pub paper: PaperNode,
    pub authors: Vec<AuthorRef>,
    pub links: PaperLinks,
    pub cites_in_library: Vec<LinkedPaper>,
    pub cites_elsewhere: Vec<LinkedPaper>,
    pub cited_by_in_library: Vec<LinkedPaper>,
    pub related: Vec<PaperHit>,
    pub methods: Vec<ConceptNode>,
    pub datasets: Vec<ConceptNode>,
    /// Methods this paper proposed (deterministic title/abstract rule).
    pub proposes: Vec<ConceptNode>,
}

/// Links of a paper.
pub fn links_of(paper: &PaperNode) -> PaperLinks {
    PaperLinks {
        doi: paper.doi.as_ref().map(|d| format!("https://doi.org/{d}")),
        arxiv: paper
            .arxiv_id
            .as_ref()
            .map(|a| format!("https://arxiv.org/abs/{a}")),
        openalex: paper
            .openalex_id
            .as_ref()
            .map(|o| format!("https://openalex.org/{o}")),
    }
}

fn authors_of(graph: &PaperGraph, paper: &PaperNode) -> Vec<AuthorRef> {
    let printed: Vec<String> = paper
        .authors
        .as_deref()
        .map(|a| {
            a.trim_end_matches(" et al.")
                .split(", ")
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    paper
        .author_ids
        .iter()
        .enumerate()
        .map(|(i, id)| AuthorRef {
            id: id.clone(),
            name: graph
                .author_name(id)
                .map(str::to_string)
                .or_else(|| printed.get(i).cloned())
                .unwrap_or_else(|| author_surname(id)),
        })
        .collect()
}

/// The paper page of `id`.
pub fn paper_view(graph: &PaperGraph, id: &str) -> Option<PaperView> {
    let paper = graph.paper(id)?.clone();
    let mut cites_in_library = Vec::new();
    let mut cites_elsewhere = Vec::new();
    for cited in graph.cited(id).into_iter().take(MAX_LISTED) {
        let linked = LinkedPaper {
            evidence: graph.evidence(id, &cited.id).cloned(),
            citing_file: paper.file_path.clone(),
            paper: cited.clone(),
        };
        if cited.in_library {
            cites_in_library.push(linked);
        } else {
            cites_elsewhere.push(linked);
        }
    }
    cites_elsewhere.sort_by(|a, b| {
        b.paper
            .cited_by_count
            .cmp(&a.paper.cited_by_count)
            .then(a.paper.label().cmp(&b.paper.label()))
    });
    let cited_by_in_library = graph
        .citers(id)
        .into_iter()
        .filter(|p| p.in_library)
        .take(MAX_LISTED)
        .map(|citer| LinkedPaper {
            evidence: graph.evidence(&citer.id, id).cloned(),
            citing_file: citer.file_path.clone(),
            paper: citer.clone(),
        })
        .collect();
    let proposes = graph
        .methods()
        .filter(|m| graph.proposed_in(&m.id).is_some_and(|p| p.id == id))
        .cloned()
        .collect();
    Some(PaperView {
        authors: authors_of(graph, &paper),
        links: links_of(&paper),
        cites_in_library,
        cites_elsewhere,
        cited_by_in_library,
        related: graph.related(id, MAX_RELATED),
        methods: graph.methods_of(id).into_iter().cloned().collect(),
        datasets: graph.datasets_of(id).into_iter().cloned().collect(),
        proposes,
        paper,
    })
}

/// A method or dataset page.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConceptView {
    pub concept: ConceptNode,
    /// `method` or `dataset`.
    pub kind: &'static str,
    pub papers: Vec<PaperNode>,
    /// The library paper that proposed the method, when the rule found one.
    pub proposed_in: Option<PaperNode>,
}

/// The page of a method (`kind` = `method`) or dataset, by id or label.
pub fn concept_view(graph: &PaperGraph, kind: &str, id_or_label: &str) -> Option<ConceptView> {
    let (kind, id) = match kind {
        "method" => (
            "method",
            if id_or_label.starts_with("method:") {
                id_or_label.to_string()
            } else {
                crate::research::results::canonical_id("method", id_or_label)
            },
        ),
        "dataset" => (
            "dataset",
            if id_or_label.starts_with("dataset:") {
                id_or_label.to_string()
            } else {
                crate::research::results::canonical_id("dataset", id_or_label)
            },
        ),
        _ => return None,
    };
    let concept = match kind {
        "method" => graph.method(&id)?.clone(),
        _ => graph.dataset(&id)?.clone(),
    };
    let papers = match kind {
        "method" => graph.papers_using(&id),
        _ => graph.papers_on(&id),
    }
    .into_iter()
    .cloned()
    .collect();
    Some(ConceptView {
        proposed_in: (kind == "method")
            .then(|| graph.proposed_in(&id).cloned())
            .flatten(),
        concept,
        kind,
        papers,
    })
}

/// One node of the graph view.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewNode {
    pub id: String,
    pub label: String,
    pub year: Option<i32>,
    pub in_library: bool,
    pub file_path: Option<String>,
    /// Library papers citing it.
    pub library_citers: usize,
    pub cited_by_count: Option<i64>,
    pub methods: Vec<String>,
}

/// The graph view's data: nodes (library papers first, then the most cited), edges among
/// them, and the methods for the filter.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphView {
    pub nodes: Vec<ViewNode>,
    /// `(citing, cited)` node ids.
    pub edges: Vec<(String, String)>,
    pub methods: Vec<ConceptNode>,
    /// Nodes left out beyond [`MAX_VIEW_NODES`].
    pub omitted: usize,
}

/// The graph view's data.
pub fn graph_view(graph: &PaperGraph) -> GraphView {
    let mut nodes: Vec<ViewNode> = graph
        .papers()
        .iter()
        .map(|p| ViewNode {
            id: p.id.clone(),
            label: p.label(),
            year: p.year,
            in_library: p.in_library,
            file_path: p.file_path.clone(),
            library_citers: graph.citers(&p.id).iter().filter(|c| c.in_library).count(),
            cited_by_count: p.cited_by_count,
            methods: graph
                .methods_of(&p.id)
                .iter()
                .map(|m| m.id.clone())
                .collect(),
        })
        .collect();
    nodes.sort_by(|a, b| {
        b.in_library
            .cmp(&a.in_library)
            .then(b.library_citers.cmp(&a.library_citers))
            .then(a.id.cmp(&b.id))
    });
    let omitted = nodes.len().saturating_sub(MAX_VIEW_NODES);
    nodes.truncate(MAX_VIEW_NODES);
    let kept: std::collections::HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let edges = graph
        .edges()
        .into_iter()
        .filter(|(a, b)| kept.contains(a) && kept.contains(b))
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    GraphView {
        nodes,
        edges,
        methods: graph.methods().cloned().collect(),
        omitted,
    }
}

/// `find_papers` over the graph.
pub fn find(graph: &PaperGraph, filter: &PaperFilter, limit: usize) -> Vec<PaperNode> {
    graph
        .find_papers(filter)
        .into_iter()
        .take(limit)
        .cloned()
        .collect()
}

/// A lineage path, for the tool.
pub fn lineage(graph: &PaperGraph, from: &str, to: &str) -> Option<Vec<PathStep>> {
    graph.lineage(from, to, 6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research::citations::graph::tests::sample;

    #[test]
    fn the_paper_page_splits_citations_by_library_and_lists_evidence() {
        let g = sample();
        let view = paper_view(&g, "paper:arxiv:2406.06484").unwrap();
        assert_eq!(view.cites_in_library.len(), 1);
        assert_eq!(view.cites_elsewhere.len(), 2);
        assert_eq!(view.cited_by_in_library.len(), 1);
        assert!(view.cited_by_in_library[0].evidence.is_some());
        assert_eq!(view.authors[0].name, "Songlin Yang");
        assert_eq!(
            view.links.arxiv.as_deref(),
            Some("https://arxiv.org/abs/2406.06484")
        );
        assert_eq!(view.proposes.len(), 1);
        assert!(view.methods.iter().any(|m| m.label == "DeltaNet"));
        assert!(paper_view(&g, "paper:none").is_none());
    }

    #[test]
    fn concept_pages_and_the_graph_view() {
        let g = sample();
        let method = concept_view(&g, "method", "DeltaNet").unwrap();
        assert_eq!(method.papers.len(), 2);
        assert_eq!(
            method.proposed_in.map(|p| p.id).as_deref(),
            Some("paper:arxiv:2406.06484")
        );
        let dataset = concept_view(&g, "dataset", "dataset:wikitext103").unwrap();
        assert_eq!(dataset.papers.len(), 1);
        assert!(concept_view(&g, "metric", "x").is_none());
        let view = graph_view(&g);
        assert_eq!(view.nodes.len(), 7);
        assert!(view.nodes[..4].iter().all(|n| n.in_library));
        assert_eq!(view.edges.len(), 9);
        assert_eq!(view.omitted, 0);
    }
}
