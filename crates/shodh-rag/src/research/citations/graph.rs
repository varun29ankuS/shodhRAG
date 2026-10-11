//! The citation graph in memory: papers, authors, methods and datasets with their edges,
//! the queries the tools and pages ask of it, and personalized PageRank over it.
//!
//! The snapshot is derived: the statement store holds the graph (Paper, Author, Venue and
//! Method statements), and [`PaperGraph`] is rebuilt from those statements after each
//! build or when first needed. It is small (papers in the library, the works they cite and
//! the methods and datasets of their results), so every query here is a scan or a short
//! search over adjacency lists.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::segment::EntryRegion;
use super::text::{normalize_surname, normalize_title};

/// One paper node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperNode {
    /// Entity id (`paper:arxiv:2406.06484`, `paper:doi:…`, `paper:openalex:W…`,
    /// `paper:title:…`).
    pub id: String,
    pub title: Option<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub openalex_id: Option<String>,
    /// The author list as printed.
    pub authors: Option<String>,
    /// Author entity ids, in order.
    #[serde(default)]
    pub author_ids: Vec<String>,
    pub venue: Option<String>,
    pub in_library: bool,
    /// The PDF, for a paper in the library (as spelled on disk).
    pub file_path: Option<String>,
    pub cited_by_count: Option<i64>,
    /// Statement id of the node (for provenance links).
    pub statement_id: Option<String>,
}

impl PaperNode {
    /// The title, or a placeholder naming the file or identifier.
    pub fn label(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        if let Some(path) = &self.file_path {
            return crate::research::file_name(path);
        }
        self.arxiv_id
            .as_ref()
            .map(|a| format!("arXiv:{a}"))
            .or_else(|| self.doi.clone())
            .unwrap_or_else(|| self.id.clone())
    }
}

/// Where a library paper cites a work: the reference entry as printed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationEvidence {
    pub text: String,
    pub page: Option<u32>,
    #[serde(default)]
    pub regions: Vec<EntryRegion>,
}

/// A method or dataset node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConceptNode {
    /// `method:…` or `dataset:…` (the canonical ids of Result statements).
    pub id: String,
    pub label: String,
}

/// The graph.
#[derive(Debug, Clone, Default)]
pub struct PaperGraph {
    papers: Vec<PaperNode>,
    by_id: HashMap<String, usize>,
    /// `cites[i]`: the papers paper `i` cites (only library papers have references).
    cites: Vec<BTreeSet<usize>>,
    /// `cited_by[j]`: the papers citing `j`.
    cited_by: Vec<BTreeSet<usize>>,
    evidence: HashMap<(usize, usize), CitationEvidence>,
    authors: BTreeMap<String, String>,
    methods: BTreeMap<String, ConceptNode>,
    datasets: BTreeMap<String, ConceptNode>,
    uses_method: Vec<BTreeSet<String>>,
    evaluated_on: Vec<BTreeSet<String>>,
    proposed_in: BTreeMap<String, usize>,
}

/// Inputs of [`PaperGraph::new`].
#[derive(Debug, Clone, Default)]
pub struct GraphParts {
    pub papers: Vec<PaperNode>,
    /// `(citing id, cited id, evidence)`.
    pub cites: Vec<(String, String, Option<CitationEvidence>)>,
    /// Author id → name.
    pub authors: BTreeMap<String, String>,
    pub methods: Vec<ConceptNode>,
    pub datasets: Vec<ConceptNode>,
    /// `(paper id, method id)`.
    pub uses_method: Vec<(String, String)>,
    /// `(paper id, dataset id)`.
    pub evaluated_on: Vec<(String, String)>,
    /// `(method id, paper id)`.
    pub proposed_in: Vec<(String, String)>,
}

/// A paper with what the caller asked about it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperHit {
    pub paper: PaperNode,
    /// Number of library papers citing it, shared references, path position… (per query).
    pub count: usize,
}

/// One step of a lineage path.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathStep {
    pub paper: PaperNode,
    /// How this step relates to the previous one: `cites` (the previous paper cites this
    /// one) or `cited_by` (this one cites the previous). `None` for the first step.
    pub relation: Option<&'static str>,
}

/// Graph size, for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSize {
    pub papers: usize,
    pub library_papers: usize,
    pub cites: usize,
    pub authors: usize,
    pub methods: usize,
    pub datasets: usize,
}

/// What a query activates in the graph.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Seeds {
    /// Node ids with weights (papers, `author:`, `method:`, `dataset:`).
    pub nodes: Vec<(String, f64)>,
    /// The query asks about citations or lineage.
    pub cue: bool,
}

static CUE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:cit(?:e|es|ed|ing|ation|ations)|references?|build(?:s|ing)? on|built on|prior work|related work|lineage|influen\w*|predecessors?|follow-?ups?|extends?|based on|everyone|in my library)\b").ok()
});
static ARXIV_IN_QUERY: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\b(\d{4}\.\d{4,5})(?:v\d+)?\b").ok());

/// A library file is ranked only when its score is at least this share of the best one.
pub const RELATIVE_FLOOR: f64 = 0.05;

/// Labels too generic to seed by (they occur in ordinary questions).
const GENERIC_LABELS: &[&str] = &[
    "ours", "our", "model", "method", "baseline", "base", "large", "small", "all", "average",
    "avg", "mean", "total", "test", "train", "dev", "full", "none", "standard",
];

/// The short name a title gives its method (`KAN: Kolmogorov-Arnold Networks` → `KAN`):
/// one to three words before a colon, at most 30 characters.
pub fn title_alias(title: &str) -> Option<String> {
    let (head, rest) = title.split_once(':')?;
    let head = head.trim();
    let words = head.split_whitespace().count();
    (!rest.trim().is_empty()
        && (1..=3).contains(&words)
        && head.chars().count() <= 30
        && head.chars().any(char::is_uppercase))
    .then(|| head.to_string())
}

fn contains_phrase(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let hay = format!(" {haystack} ");
    hay.contains(&format!(" {needle} "))
}

impl PaperGraph {
    /// Builds the graph; edges naming unknown nodes are dropped.
    pub fn new(parts: GraphParts) -> Self {
        let mut g = PaperGraph::default();
        for paper in parts.papers {
            if g.by_id.contains_key(&paper.id) {
                continue;
            }
            g.by_id.insert(paper.id.clone(), g.papers.len());
            g.papers.push(paper);
        }
        let n = g.papers.len();
        g.cites = vec![BTreeSet::new(); n];
        g.cited_by = vec![BTreeSet::new(); n];
        g.uses_method = vec![BTreeSet::new(); n];
        g.evaluated_on = vec![BTreeSet::new(); n];
        for (from, to, evidence) in parts.cites {
            let (Some(&a), Some(&b)) = (g.by_id.get(&from), g.by_id.get(&to)) else {
                continue;
            };
            if a == b {
                continue;
            }
            g.cites[a].insert(b);
            g.cited_by[b].insert(a);
            if let Some(e) = evidence {
                g.evidence.entry((a, b)).or_insert(e);
            }
        }
        g.authors = parts.authors;
        for m in parts.methods {
            g.methods.insert(m.id.clone(), m);
        }
        for d in parts.datasets {
            g.datasets.insert(d.id.clone(), d);
        }
        for (paper, method) in parts.uses_method {
            if let (Some(&i), true) = (g.by_id.get(&paper), g.methods.contains_key(&method)) {
                g.uses_method[i].insert(method);
            }
        }
        for (paper, dataset) in parts.evaluated_on {
            if let (Some(&i), true) = (g.by_id.get(&paper), g.datasets.contains_key(&dataset)) {
                g.evaluated_on[i].insert(dataset);
            }
        }
        for (method, paper) in parts.proposed_in {
            if let (Some(&i), true) = (g.by_id.get(&paper), g.methods.contains_key(&method)) {
                g.proposed_in.insert(method, i);
            }
        }
        g
    }

    pub fn size(&self) -> GraphSize {
        GraphSize {
            papers: self.papers.len(),
            library_papers: self.papers.iter().filter(|p| p.in_library).count(),
            cites: self.cites.iter().map(BTreeSet::len).sum(),
            authors: self.authors.len(),
            methods: self.methods.len(),
            datasets: self.datasets.len(),
        }
    }

    pub fn papers(&self) -> &[PaperNode] {
        &self.papers
    }

    pub fn paper(&self, id: &str) -> Option<&PaperNode> {
        self.by_id.get(id).map(|&i| &self.papers[i])
    }

    pub fn method(&self, id: &str) -> Option<&ConceptNode> {
        self.methods.get(id)
    }

    pub fn dataset(&self, id: &str) -> Option<&ConceptNode> {
        self.datasets.get(id)
    }

    pub fn author_name(&self, id: &str) -> Option<&str> {
        self.authors.get(id).map(String::as_str)
    }

    /// Every edge as `(citing id, cited id)`.
    pub fn edges(&self) -> Vec<(&str, &str)> {
        self.cites
            .iter()
            .enumerate()
            .flat_map(|(a, targets)| {
                targets
                    .iter()
                    .map(move |&b| (self.papers[a].id.as_str(), self.papers[b].id.as_str()))
            })
            .collect()
    }

    /// The library paper of a file (any spelling of its path).
    pub fn paper_for_file(&self, path: &str) -> Option<&PaperNode> {
        let key = crate::research::path_key(path);
        self.papers.iter().find(|p| {
            p.file_path
                .as_deref()
                .is_some_and(|f| crate::research::path_key(f) == key)
        })
    }

    /// Finds a paper by entity id, file path, arXiv id, DOI or (near-exact) title.
    pub fn find(&self, query: &str) -> Option<&PaperNode> {
        let q = query.trim();
        if q.is_empty() {
            return None;
        }
        if let Some(p) = self.paper(q) {
            return Some(p);
        }
        if let Some(p) = self.paper_for_file(q) {
            return Some(p);
        }
        if let Some(id) = crate::harness::web::papers::normalize_arxiv_id(q) {
            if let Some(p) = self
                .papers
                .iter()
                .find(|p| p.arxiv_id.as_deref() == Some(&id))
            {
                return Some(p);
            }
        }
        if let Some(doi) = crate::harness::web::papers::normalize_doi(q) {
            if let Some(p) = self.papers.iter().find(|p| p.doi.as_deref() == Some(&doi)) {
                return Some(p);
            }
        }
        let norm = normalize_title(q);
        let exact = |p: &&PaperNode| {
            p.title
                .as_deref()
                .is_some_and(|t| normalize_title(t) == norm)
                || p.title
                    .as_deref()
                    .and_then(title_alias)
                    .is_some_and(|a| normalize_title(&a) == norm)
        };
        // Library papers first, then the most cited.
        let mut candidates: Vec<&PaperNode> = self.papers.iter().filter(exact).collect();
        candidates.sort_by_key(|p| (!p.in_library, std::cmp::Reverse(self.cited_by_count(p))));
        if let Some(p) = candidates.first() {
            return Some(p);
        }
        self.papers
            .iter()
            .filter_map(|p| {
                let t = p.title.as_deref()?;
                let s = super::text::title_similarity(t, q);
                (s >= 0.9).then_some((p, s))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(p, _)| p)
    }

    fn index_of(&self, paper: &PaperNode) -> Option<usize> {
        self.by_id.get(&paper.id).copied()
    }

    fn cited_by_count(&self, paper: &PaperNode) -> usize {
        self.index_of(paper)
            .map(|i| self.cited_by[i].len())
            .unwrap_or(0)
    }

    /// The works a paper cites.
    pub fn cited(&self, id: &str) -> Vec<&PaperNode> {
        self.by_id
            .get(id)
            .map(|&i| self.cites[i].iter().map(|&j| &self.papers[j]).collect())
            .unwrap_or_default()
    }

    /// The library papers citing a paper.
    pub fn citers(&self, id: &str) -> Vec<&PaperNode> {
        self.by_id
            .get(id)
            .map(|&i| self.cited_by[i].iter().map(|&j| &self.papers[j]).collect())
            .unwrap_or_default()
    }

    /// Where `citing` cites `cited`, as printed.
    pub fn evidence(&self, citing: &str, cited: &str) -> Option<&CitationEvidence> {
        let a = *self.by_id.get(citing)?;
        let b = *self.by_id.get(cited)?;
        self.evidence.get(&(a, b))
    }

    /// The works both papers cite.
    pub fn shared_references(&self, a: &str, b: &str) -> Vec<&PaperNode> {
        let (Some(&i), Some(&j)) = (self.by_id.get(a), self.by_id.get(b)) else {
            return Vec::new();
        };
        self.cites[i]
            .intersection(&self.cites[j])
            .map(|&k| &self.papers[k])
            .collect()
    }

    /// Library papers sharing references with `id`, most shared first (`count` = shared).
    pub fn related(&self, id: &str, limit: usize) -> Vec<PaperHit> {
        let Some(&i) = self.by_id.get(id) else {
            return Vec::new();
        };
        let mut hits: Vec<PaperHit> = (0..self.papers.len())
            .filter(|&j| j != i && self.papers[j].in_library)
            .filter_map(|j| {
                let shared = self.cites[i].intersection(&self.cites[j]).count();
                let direct = self.cites[i].contains(&j) || self.cites[j].contains(&i);
                (shared > 0 || direct).then(|| PaperHit {
                    paper: self.papers[j].clone(),
                    count: shared,
                })
            })
            .collect();
        hits.sort_by(|a, b| b.count.cmp(&a.count).then(a.paper.id.cmp(&b.paper.id)));
        hits.truncate(limit);
        hits
    }

    /// The works cited by the most library papers (`count` = citing library papers):
    /// what the library builds on.
    pub fn most_cited(&self, limit: usize, min_citers: usize) -> Vec<PaperHit> {
        let mut hits: Vec<PaperHit> = self
            .cited_by
            .iter()
            .enumerate()
            .filter(|(_, citers)| citers.len() >= min_citers.max(1))
            .map(|(j, citers)| PaperHit {
                paper: self.papers[j].clone(),
                count: citers.len(),
            })
            .collect();
        hits.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(b.paper.cited_by_count.cmp(&a.paper.cited_by_count))
                .then(a.paper.id.cmp(&b.paper.id))
        });
        hits.truncate(limit);
        hits
    }

    /// A shortest chain of citations between two papers, following edges in either
    /// direction; library papers are preferred as intermediates (they are tried first).
    pub fn lineage(&self, from: &str, to: &str, max_hops: usize) -> Option<Vec<PathStep>> {
        let start = *self.by_id.get(from)?;
        let goal = *self.by_id.get(to)?;
        let mut previous: HashMap<usize, (usize, &'static str)> = HashMap::new();
        let mut depth: HashMap<usize, usize> = HashMap::from([(start, 0)]);
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            if node == goal {
                break;
            }
            let d = depth[&node];
            if d >= max_hops {
                continue;
            }
            let mut next: Vec<(usize, &'static str)> = self.cites[node]
                .iter()
                .map(|&m| (m, "cites"))
                .chain(self.cited_by[node].iter().map(|&m| (m, "cited_by")))
                .collect();
            next.sort_by_key(|(m, _)| (!self.papers[*m].in_library, *m));
            for (m, relation) in next {
                if depth.contains_key(&m) {
                    continue;
                }
                depth.insert(m, d + 1);
                previous.insert(m, (node, relation));
                queue.push_back(m);
            }
        }
        if !depth.contains_key(&goal) {
            return None;
        }
        // Walk back from the goal; each step keeps how it relates to the step before it.
        let mut steps: Vec<(usize, Option<&'static str>)> = Vec::new();
        let mut at = goal;
        while let Some(&(prev, relation)) = previous.get(&at) {
            steps.push((at, Some(relation)));
            at = prev;
        }
        steps.push((start, None));
        steps.reverse();
        Some(
            steps
                .into_iter()
                .map(|(i, relation)| PathStep {
                    paper: self.papers[i].clone(),
                    relation,
                })
                .collect(),
        )
    }

    /// Methods a paper uses (from its results and its headings/captions).
    pub fn methods_of(&self, id: &str) -> Vec<&ConceptNode> {
        self.by_id
            .get(id)
            .map(|&i| {
                self.uses_method[i]
                    .iter()
                    .filter_map(|m| self.methods.get(m))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Datasets a paper is evaluated on.
    pub fn datasets_of(&self, id: &str) -> Vec<&ConceptNode> {
        self.by_id
            .get(id)
            .map(|&i| {
                self.evaluated_on[i]
                    .iter()
                    .filter_map(|d| self.datasets.get(d))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The library paper that proposed a method, when one was found.
    pub fn proposed_in(&self, method_id: &str) -> Option<&PaperNode> {
        self.proposed_in.get(method_id).map(|&i| &self.papers[i])
    }

    /// Papers using a method (by id).
    pub fn papers_using(&self, method_id: &str) -> Vec<&PaperNode> {
        self.uses_method
            .iter()
            .enumerate()
            .filter(|(_, m)| m.contains(method_id))
            .map(|(i, _)| &self.papers[i])
            .collect()
    }

    /// Papers evaluated on a dataset (by id).
    pub fn papers_on(&self, dataset_id: &str) -> Vec<&PaperNode> {
        self.evaluated_on
            .iter()
            .enumerate()
            .filter(|(_, d)| d.contains(dataset_id))
            .map(|(i, _)| &self.papers[i])
            .collect()
    }

    /// All method and dataset nodes.
    pub fn methods(&self) -> impl Iterator<Item = &ConceptNode> {
        self.methods.values()
    }

    pub fn datasets(&self) -> impl Iterator<Item = &ConceptNode> {
        self.datasets.values()
    }

    /// Papers matching every given filter: a method or dataset (label or id), an author
    /// (surname or name), a year range. Library papers first, then by year (newest).
    pub fn find_papers(&self, filter: &PaperFilter) -> Vec<&PaperNode> {
        let method = filter
            .method
            .as_deref()
            .map(|m| crate::research::results::canonical_id("method", m));
        let dataset = filter
            .dataset
            .as_deref()
            .map(|d| crate::research::results::canonical_id("dataset", d));
        let author = filter.author.as_deref().map(|a| {
            a.split_whitespace()
                .last()
                .map(normalize_surname)
                .unwrap_or_default()
        });
        let mut out: Vec<&PaperNode> = self
            .papers
            .iter()
            .enumerate()
            .filter(|(i, p)| {
                if filter.in_library_only && !p.in_library {
                    return false;
                }
                if let Some(m) = &method {
                    let uses =
                        self.uses_method[*i].contains(m) || self.proposed_in.get(m) == Some(i);
                    if !uses {
                        return false;
                    }
                }
                if let Some(d) = &dataset {
                    if !self.evaluated_on[*i].contains(d) {
                        return false;
                    }
                }
                if let Some(a) = author.as_deref().filter(|a| !a.is_empty()) {
                    let by_id = p.author_ids.iter().any(|id| author_surname(id) == a);
                    let printed = p.authors.as_deref().is_some_and(|names| {
                        names
                            .split([',', ';'])
                            .flat_map(|n| n.split(" and "))
                            .any(|n| n.split_whitespace().map(normalize_surname).any(|w| w == a))
                    });
                    if !by_id && !printed {
                        return false;
                    }
                }
                if let Some(from) = filter.year_from {
                    if p.year.is_none_or(|y| y < from) {
                        return false;
                    }
                }
                if let Some(to) = filter.year_to {
                    if p.year.is_none_or(|y| y > to) {
                        return false;
                    }
                }
                true
            })
            .map(|(_, p)| p)
            .collect();
        out.sort_by(|a, b| {
            b.in_library
                .cmp(&a.in_library)
                .then(b.year.cmp(&a.year))
                .then(a.id.cmp(&b.id))
        });
        out
    }

    /// What a query names in the graph: arXiv ids, titles and their short names, method
    /// and dataset labels, author surnames; and whether it asks about citations at all.
    pub fn seeds(&self, query: &str) -> Seeds {
        let cue = CUE.as_ref().is_some_and(|r| r.is_match(query));
        let norm = normalize_title(query);
        let mut nodes: BTreeMap<String, f64> = BTreeMap::new();
        if let Some(re) = ARXIV_IN_QUERY.as_ref() {
            for c in re.captures_iter(query) {
                if let Some(p) = c.get(1).and_then(|m| {
                    self.papers
                        .iter()
                        .find(|p| p.arxiv_id.as_deref() == Some(m.as_str()))
                }) {
                    nodes.insert(p.id.clone(), 1.0);
                }
            }
        }
        for p in &self.papers {
            let Some(title) = p.title.as_deref() else {
                continue;
            };
            let t = normalize_title(title);
            if t.split(' ').count() >= 3 && contains_phrase(&norm, &t) {
                *nodes.entry(p.id.clone()).or_insert(0.0) += 1.0;
            } else if let Some(alias) = title_alias(title) {
                let a = normalize_title(&alias);
                if a.len() >= 3
                    && !GENERIC_LABELS.contains(&a.as_str())
                    && contains_phrase(&norm, &a)
                {
                    *nodes.entry(p.id.clone()).or_insert(0.0) += 0.8;
                }
            }
        }
        for concept in self.methods.values().chain(self.datasets.values()) {
            let l = normalize_title(&concept.label);
            if l.len() >= 3 && !GENERIC_LABELS.contains(&l.as_str()) && contains_phrase(&norm, &l) {
                nodes.insert(concept.id.clone(), 0.8);
            }
        }
        // Surnames written with a capital in the query.
        let capitalised: HashSet<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '-')
            .filter(|w| w.chars().next().is_some_and(char::is_uppercase) && w.chars().count() >= 3)
            .map(normalize_surname)
            .collect();
        for id in self.authors.keys() {
            if capitalised.contains(&author_surname(id)) {
                nodes.insert(id.clone(), 0.6);
            }
        }
        Seeds {
            nodes: nodes.into_iter().collect(),
            cue,
        }
    }

    /// Personalized PageRank from `seeds` over papers, authors, methods and datasets
    /// (edges undirected and weighted: cites 1.0, uses method 0.7, evaluated on 0.5,
    /// authored by 0.5). Returns every node's score, summing to one.
    pub fn personalized_pagerank(
        &self,
        seeds: &[(String, f64)],
        damping: f64,
        iterations: usize,
    ) -> HashMap<String, f64> {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        let node = |id: &str, index: &mut HashMap<String, usize>, names: &mut Vec<String>| {
            *index.entry(id.to_string()).or_insert_with(|| {
                names.push(id.to_string());
                names.len() - 1
            })
        };
        let mut edges: Vec<(usize, usize, f64)> = Vec::new();
        for p in &self.papers {
            node(&p.id, &mut index, &mut names);
        }
        for (a, targets) in self.cites.iter().enumerate() {
            for &b in targets {
                edges.push((a, b, 1.0));
            }
        }
        for (i, p) in self.papers.iter().enumerate() {
            for author in &p.author_ids {
                let k = node(author, &mut index, &mut names);
                edges.push((i, k, 0.5));
            }
            for m in &self.uses_method[i] {
                let k = node(m, &mut index, &mut names);
                edges.push((i, k, 0.7));
            }
            for d in &self.evaluated_on[i] {
                let k = node(d, &mut index, &mut names);
                edges.push((i, k, 0.5));
            }
        }
        for (m, &i) in &self.proposed_in {
            let k = node(m, &mut index, &mut names);
            edges.push((i, k, 1.0));
        }
        let n = names.len();
        if n == 0 {
            return HashMap::new();
        }
        let mut adjacency: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        for (a, b, w) in edges {
            adjacency[a].push((b, w));
            adjacency[b].push((a, w));
        }
        let out_weight: Vec<f64> = adjacency
            .iter()
            .map(|list| list.iter().map(|(_, w)| w).sum())
            .collect();
        let mut teleport = vec![0.0f64; n];
        let total: f64 = seeds
            .iter()
            .filter(|(id, _)| index.contains_key(id))
            .map(|(_, w)| w.max(0.0))
            .sum();
        if total <= 0.0 {
            return HashMap::new();
        }
        for (id, w) in seeds {
            if let Some(&k) = index.get(id) {
                teleport[k] += w.max(0.0) / total;
            }
        }
        let mut rank = teleport.clone();
        for _ in 0..iterations {
            let mut next = vec![0.0f64; n];
            let mut dangling = 0.0;
            for (a, list) in adjacency.iter().enumerate() {
                if out_weight[a] <= 0.0 {
                    dangling += rank[a];
                    continue;
                }
                for &(b, w) in list {
                    next[b] += damping * rank[a] * w / out_weight[a];
                }
            }
            for (k, value) in next.iter_mut().enumerate() {
                *value += ((1.0 - damping) + damping * dangling) * teleport[k];
            }
            let delta: f64 = next.iter().zip(&rank).map(|(x, y)| (x - y).abs()).sum();
            rank = next;
            if delta < 1e-9 {
                break;
            }
        }
        names.into_iter().zip(rank).collect()
    }

    /// The library files a query points at, best first, or `None` when the query names
    /// nothing in the graph and asks nothing about citations (the search is then left
    /// alone). Without named nodes but with a citation cue, every library paper is a seed.
    /// A cited work's score also counts for the library papers citing it (half each).
    pub fn rank_library_files(&self, query: &str, limit: usize) -> Option<Vec<String>> {
        let seeds = self.seeds(query);
        let nodes = if seeds.nodes.is_empty() {
            if !seeds.cue {
                return None;
            }
            self.papers
                .iter()
                .filter(|p| p.in_library)
                .map(|p| (p.id.clone(), 1.0))
                .collect()
        } else {
            seeds.nodes
        };
        let scores = self.personalized_pagerank(&nodes, 0.85, 50);
        if scores.is_empty() {
            return None;
        }
        let mut by_file: HashMap<usize, f64> = HashMap::new();
        for (i, p) in self.papers.iter().enumerate() {
            let score = scores.get(&p.id).copied().unwrap_or(0.0);
            if score <= 0.0 {
                continue;
            }
            if p.in_library {
                *by_file.entry(i).or_insert(0.0) += score;
            }
            for &citer in &self.cited_by[i] {
                if self.papers[citer].in_library && !p.in_library {
                    *by_file.entry(citer).or_insert(0.0) += score * 0.5;
                }
            }
        }
        let best = by_file.values().copied().fold(0.0f64, f64::max);
        // Papers reached only through long chains carry almost nothing; keep the list to
        // what the query is about.
        let floor = best * RELATIVE_FLOOR;
        let mut ranked: Vec<(usize, f64)> = by_file
            .into_iter()
            .filter(|(_, s)| *s > 0.0 && *s >= floor)
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let files: Vec<String> = ranked
            .into_iter()
            .filter_map(|(i, _)| self.papers[i].file_path.clone())
            .take(limit)
            .collect();
        (!files.is_empty()).then_some(files)
    }
}

/// Filters of [`PaperGraph::find_papers`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperFilter {
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub dataset: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub year_from: Option<i32>,
    #[serde(default)]
    pub year_to: Option<i32>,
    #[serde(default)]
    pub in_library_only: bool,
}

/// The surname part of an author entity id (`author:yang-s` → `yang`).
pub fn author_surname(id: &str) -> String {
    id.strip_prefix("author:")
        .unwrap_or(id)
        .rsplit_once('-')
        .map(|(surname, _)| surname.to_string())
        .unwrap_or_else(|| id.strip_prefix("author:").unwrap_or(id).to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn node(id: &str, title: &str, year: i32, library: bool) -> PaperNode {
        PaperNode {
            id: id.to_string(),
            title: Some(title.to_string()),
            year: Some(year),
            doi: None,
            arxiv_id: id.strip_prefix("paper:arxiv:").map(str::to_string),
            openalex_id: None,
            authors: None,
            author_ids: Vec::new(),
            venue: None,
            in_library: library,
            file_path: library.then(|| format!("C:/papers/{}.pdf", id.replace(':', "_"))),
            cited_by_count: None,
            statement_id: None,
        }
    }

    /// Three library papers on linear attention citing shared foundations.
    pub(crate) fn sample() -> PaperGraph {
        let fwp = node(
            "paper:arxiv:2102.11174",
            "Linear Transformers Are Secretly Fast Weight Programmers",
            2021,
            true,
        );
        let mut delta = node(
            "paper:arxiv:2406.06484",
            "Parallelizing Linear Transformers with the Delta Rule over Sequence Length",
            2024,
            true,
        );
        delta.author_ids = vec!["author:yang-s".into(), "author:kim-y".into()];
        let neg = node(
            "paper:arxiv:2411.12537",
            "Unlocking State-Tracking in Linear RNNs Through Negative Eigenvalues",
            2024,
            true,
        );
        let kan = node(
            "paper:arxiv:2404.19756",
            "KAN: Kolmogorov-Arnold Networks",
            2024,
            true,
        );
        let attention = node(
            "paper:arxiv:1706.03762",
            "Attention Is All You Need",
            2017,
            false,
        );
        let katharopoulos = node(
            "paper:arxiv:2006.16236",
            "Transformers are RNNs: Fast Autoregressive Transformers with Linear Attention",
            2020,
            false,
        );
        let lstm = node(
            "paper:doi:10.1162/neco.1997.9.8.1735",
            "Long Short-Term Memory",
            1997,
            false,
        );
        let e = |citing: &str, cited: &str| (citing.to_string(), cited.to_string(), None);
        PaperGraph::new(GraphParts {
            papers: vec![fwp, delta, neg, kan, attention, katharopoulos, lstm],
            cites: vec![
                e("paper:arxiv:2102.11174", "paper:arxiv:1706.03762"),
                e("paper:arxiv:2102.11174", "paper:arxiv:2006.16236"),
                e("paper:arxiv:2102.11174", "paper:doi:10.1162/neco.1997.9.8.1735"),
                e("paper:arxiv:2406.06484", "paper:arxiv:2102.11174"),
                e("paper:arxiv:2406.06484", "paper:arxiv:1706.03762"),
                e("paper:arxiv:2406.06484", "paper:arxiv:2006.16236"),
                (
                    "paper:arxiv:2411.12537".to_string(),
                    "paper:arxiv:2406.06484".to_string(),
                    Some(CitationEvidence {
                        text: "S. Yang, B. Wang, Y. Zhang, Y. Shen, and Y. Kim. Parallelizing linear transformers with the delta rule over sequence length. In NeurIPS, 2024.".into(),
                        page: Some(12),
                        regions: vec![],
                    }),
                ),
                e("paper:arxiv:2411.12537", "paper:arxiv:2006.16236"),
                e("paper:arxiv:2404.19756", "paper:doi:10.1162/neco.1997.9.8.1735"),
            ],
            authors: BTreeMap::from([
                ("author:yang-s".to_string(), "Songlin Yang".to_string()),
                ("author:kim-y".to_string(), "Yoon Kim".to_string()),
            ]),
            methods: vec![
                ConceptNode { id: "method:deltanet".into(), label: "DeltaNet".into() },
                ConceptNode { id: "method:mamba".into(), label: "Mamba".into() },
            ],
            datasets: vec![ConceptNode { id: "dataset:wikitext103".into(), label: "WikiText-103".into() }],
            uses_method: vec![
                ("paper:arxiv:2406.06484".into(), "method:deltanet".into()),
                ("paper:arxiv:2406.06484".into(), "method:mamba".into()),
                ("paper:arxiv:2411.12537".into(), "method:deltanet".into()),
                ("paper:arxiv:2411.12537".into(), "method:mamba".into()),
            ],
            evaluated_on: vec![("paper:arxiv:2406.06484".into(), "dataset:wikitext103".into())],
            proposed_in: vec![("method:deltanet".into(), "paper:arxiv:2406.06484".into())],
        })
    }

    #[test]
    fn neighbours_citers_shared_references_and_related_papers() {
        let g = sample();
        assert_eq!(g.size().library_papers, 4);
        assert_eq!(g.size().cites, 9);
        let citers: Vec<&str> = g
            .citers("paper:arxiv:2406.06484")
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(citers, ["paper:arxiv:2411.12537"]);
        let shared: Vec<&str> = g
            .shared_references("paper:arxiv:2102.11174", "paper:arxiv:2406.06484")
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(shared, ["paper:arxiv:1706.03762", "paper:arxiv:2006.16236"]);
        let related = g.related("paper:arxiv:2102.11174", 5);
        assert_eq!(related[0].paper.id, "paper:arxiv:2406.06484");
        assert_eq!(related[0].count, 2);
        assert!(g
            .evidence("paper:arxiv:2411.12537", "paper:arxiv:2406.06484")
            .is_some());
    }

    #[test]
    fn what_the_library_builds_on_is_the_most_cited_work() {
        let g = sample();
        let top = g.most_cited(3, 2);
        assert_eq!(top[0].paper.id, "paper:arxiv:2006.16236");
        assert_eq!(top[0].count, 3);
        assert!(top.iter().all(|h| h.count >= 2));
    }

    #[test]
    fn lineage_follows_citations_in_either_direction() {
        let g = sample();
        let path = g
            .lineage("paper:arxiv:2411.12537", "paper:arxiv:2102.11174", 4)
            .unwrap();
        let ids: Vec<&str> = path.iter().map(|s| s.paper.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "paper:arxiv:2411.12537",
                "paper:arxiv:2406.06484",
                "paper:arxiv:2102.11174"
            ]
        );
        assert_eq!(path[1].relation, Some("cites"));
        let back = g
            .lineage("paper:arxiv:2404.19756", "paper:arxiv:2102.11174", 4)
            .unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[2].relation, Some("cited_by"));
        assert!(g
            .lineage("paper:arxiv:2404.19756", "paper:arxiv:2411.12537", 1)
            .is_none());
    }

    #[test]
    fn papers_are_found_by_id_title_alias_method_author_and_year() {
        let g = sample();
        assert_eq!(
            g.find("2406.06484v2").map(|p| p.id.as_str()),
            Some("paper:arxiv:2406.06484")
        );
        assert_eq!(
            g.find("KAN").map(|p| p.id.as_str()),
            Some("paper:arxiv:2404.19756")
        );
        assert_eq!(
            g.find("long short-term memory").map(|p| p.year),
            Some(Some(1997))
        );
        let by_method = g.find_papers(&PaperFilter {
            method: Some("DeltaNet".into()),
            ..PaperFilter::default()
        });
        assert_eq!(by_method.len(), 2);
        let by_author = g.find_papers(&PaperFilter {
            author: Some("Yoon Kim".into()),
            ..PaperFilter::default()
        });
        assert_eq!(by_author.len(), 1);
        let old = g.find_papers(&PaperFilter {
            year_to: Some(2020),
            ..PaperFilter::default()
        });
        assert_eq!(old.len(), 3);
        assert_eq!(
            g.proposed_in("method:deltanet").map(|p| p.id.as_str()),
            Some("paper:arxiv:2406.06484")
        );
    }

    #[test]
    fn pagerank_from_a_method_ranks_the_papers_around_it() {
        let g = sample();
        let scores = g.personalized_pagerank(&[("method:deltanet".to_string(), 1.0)], 0.85, 100);
        let total: f64 = scores.values().sum();
        assert!((total - 1.0).abs() < 1e-6, "{total}");
        let s = |id: &str| scores.get(id).copied().unwrap_or(0.0);
        assert!(s("paper:arxiv:2406.06484") > s("paper:arxiv:2102.11174"));
        assert!(s("paper:arxiv:2411.12537") > s("paper:arxiv:2404.19756"));
        assert!(s("paper:arxiv:2406.06484") > 10.0 * s("paper:arxiv:2404.19756"));
        assert!(g
            .personalized_pagerank(&[("nowhere".to_string(), 1.0)], 0.85, 10)
            .is_empty());
    }

    #[test]
    fn queries_without_graph_names_or_citation_cues_leave_search_alone() {
        let g = sample();
        assert_eq!(g.rank_library_files("how do I bake bread", 10), None);
        let files = g
            .rank_library_files("papers in my library that cite DeltaNet", 10)
            .unwrap();
        assert!(
            files[0].contains("2406.06484") || files[0].contains("2411.12537"),
            "{files:?}"
        );
        let position = |needle: &str| files.iter().position(|f| f.contains(needle));
        assert!(position("2404.19756").is_none_or(|kan| kan > position("2411.12537").unwrap_or(0)));
        let everyone = g
            .rank_library_files("what does everyone build on", 10)
            .unwrap();
        assert_eq!(everyone.len(), 4);
        let seeds = g.seeds("Compare Kim's work on WikiText-103");
        assert!(seeds.nodes.iter().any(|(id, _)| id == "author:kim-y"));
        assert!(seeds
            .nodes
            .iter()
            .any(|(id, _)| id == "dataset:wikitext103"));
        assert_eq!(
            title_alias("KAN: Kolmogorov-Arnold Networks").as_deref(),
            Some("KAN")
        );
        assert_eq!(title_alias("Attention is all you need"), None);
        assert_eq!(author_surname("author:oord-a"), "oord");
    }
}
