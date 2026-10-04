//! Assembling the graph from scans, OpenAlex matches and Result statements, and turning it
//! into ontology statements.
//!
//! Deterministic: the same inputs give the same nodes, ids and statements, so a rebuild
//! with nothing new writes nothing.
//!
//! **One node per work.** Every mention of a work (a library PDF, or a reference entry in
//! one) carries keys: its DOI, arXiv id and OpenAlex id, and for references its normalised
//! title. Mentions sharing a key are one node. A reference is also the same work as a
//! library paper when their titles are at least 0.9 similar and their years (within one)
//! and first-author surnames agree wherever both are known; this comparison is local and
//! sends nothing anywhere.
//!
//! **Node ids** prefer the arXiv id, then the DOI, then the OpenAlex id, then a hash of
//! the normalised title: `paper:arxiv:2406.06484`, `paper:doi:10.1162/…`,
//! `paper:openalex:W…`, `paper:title:<16 hex>`.
//!
//! **Methods and datasets** come only from Result statements (their method and dataset
//! ids) and from deterministic rules over headings and captions: a dataset label of the
//! library's results printed in a caption makes the paper `evaluatedOn` it, and a method
//! label printed in a heading makes it `usesMethod` it. A method is `proposedIn` a library
//! paper when the paper's title gives it as its short name (`KAN: …`) or its abstract says
//! "we propose/introduce/present … <name>", and no other library paper of the same or an
//! earlier year claims it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use sha2::{Digest, Sha256};
use shodh_ontology::{EntityRef, Extractor, ExtractorKind, Provenance, RawValue};

use super::graph::{title_alias, CitationEvidence, ConceptNode, GraphParts, PaperNode};
use super::reference::AuthorName;
use super::resolve::OpenAlexWork;
use super::scan::PaperScan;
use super::text::{normalize_title, slug, title_similarity, titles_agree};

/// Extractor version recorded on every graph statement.
pub const GRAPH_EXTRACTOR: &str = "citation-graph/1";
/// Lowest similarity at which a reference is taken to be a library paper.
pub const LOCAL_TITLE_THRESHOLD: f64 = 0.9;
/// Most authors kept per paper.
const MAX_AUTHORS: usize = 12;

/// What the build knows about one library PDF.
#[derive(Debug, Clone)]
pub struct ScanInput {
    pub scan: PaperScan,
    /// The OpenAlex work of the PDF itself, matched by its DOI or arXiv id.
    pub work: Option<OpenAlexWork>,
    /// OpenAlex works of its references, by reference index.
    pub reference_works: HashMap<usize, OpenAlexWork>,
    /// Methods of its accepted results: `(method id, label)`.
    pub result_methods: Vec<(String, String)>,
    /// Datasets of its accepted results: `(dataset id, label)`.
    pub result_datasets: Vec<(String, String)>,
}

/// One statement the graph wants in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct DesiredStatement {
    pub class: &'static str,
    pub subject: String,
    pub properties: BTreeMap<String, RawValue>,
    pub source: String,
    pub page: Option<u32>,
    pub confidence: f64,
}

impl DesiredStatement {
    /// Provenance at `now`.
    pub fn provenance(&self, now: DateTime<Utc>) -> Provenance {
        Provenance {
            source: self.source.clone(),
            generation: 0,
            page: self.page,
            span: None,
            extractor: Extractor {
                kind: ExtractorKind::Rule,
                version: GRAPH_EXTRACTOR.to_string(),
            },
            confidence: self.confidence.clamp(0.0, 1.0),
            extracted_at: now,
        }
    }
}

/// The assembled graph: statements to store and the in-memory parts.
#[derive(Debug, Clone, Default)]
pub struct Assembled {
    pub statements: Vec<DesiredStatement>,
    pub parts: GraphParts,
    /// References that identified no work (no identifier, title or year).
    pub unidentified: usize,
    /// References linked to a library paper by the local title rule.
    pub local_title_links: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Mention {
    Local(usize),
    Reference(usize, usize),
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn find(&mut self, i: usize) -> usize {
        let mut root = i;
        while self.0[root] != root {
            root = self.0[root];
        }
        let mut at = i;
        while self.0[at] != root {
            let next = self.0[at];
            self.0[at] = root;
            at = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (low, high) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0[high] = low;
        }
    }
}

/// Facts about one mention.
#[derive(Debug, Clone, Default)]
struct Facts {
    title: Option<(String, f64)>,
    year: Option<(i32, f64)>,
    doi: Option<String>,
    arxiv_id: Option<String>,
    openalex_id: Option<String>,
    authors: Vec<String>,
    et_al: bool,
    venue: Option<(String, f64)>,
    cited_by_count: Option<i64>,
    first_surname: Option<String>,
}

fn facts_of_work(work: &OpenAlexWork) -> Facts {
    Facts {
        title: Some((work.title.clone(), 0.99)),
        year: work.year.map(|y| (y, 0.99)),
        doi: work.doi.clone(),
        arxiv_id: work.arxiv_id.clone(),
        openalex_id: Some(work.id.clone()),
        first_surname: work
            .authors
            .first()
            .and_then(|a| a.split_whitespace().last())
            .map(super::text::normalize_surname),
        authors: work.authors.iter().take(MAX_AUTHORS).cloned().collect(),
        et_al: work.authors.len() > MAX_AUTHORS,
        venue: work.venue.clone().map(|v| (v, 0.95)),
        cited_by_count: work.cited_by_count,
    }
}

fn names(authors: &[AuthorName]) -> Vec<String> {
    authors
        .iter()
        .take(MAX_AUTHORS)
        .map(AuthorName::display)
        .collect()
}

/// Facts of a library PDF: its own identity, completed by its OpenAlex work.
fn facts_of_local(input: &ScanInput) -> Facts {
    let id = &input.scan.identity;
    let mut f = Facts {
        title: id.title.clone().map(|t| (t, 0.95)),
        year: id.year.map(|y| (y, 0.9)),
        doi: id.doi.clone(),
        arxiv_id: id.arxiv_id.clone(),
        authors: names(&id.authors),
        first_surname: id.authors.first().map(AuthorName::surname_key),
        ..Facts::default()
    };
    if let Some(work) = &input.work {
        let w = facts_of_work(work);
        // The work's title when it is this paper's (it has the publisher's casing and is
        // never cut off); the first page's otherwise.
        f.title = match (&f.title, w.title) {
            (Some((own, _)), Some((theirs, wt))) if titles_agree(own, &theirs) => {
                Some((theirs, wt))
            }
            (Some(own), _) => Some(own.clone()),
            (None, theirs) => theirs,
        };
        f.year = w.year.or(f.year);
        f.doi = f.doi.or(w.doi);
        f.arxiv_id = f.arxiv_id.or(w.arxiv_id);
        f.openalex_id = w.openalex_id;
        if !w.authors.is_empty() {
            f.authors = w.authors;
            f.et_al = w.et_al;
            f.first_surname = w.first_surname;
        }
        f.venue = w.venue;
        f.cited_by_count = w.cited_by_count;
    }
    f
}

fn facts_of_reference(input: &ScanInput, index: usize) -> Option<Facts> {
    let reference = input.scan.references.iter().find(|r| r.index == index)?;
    let p = &reference.parsed;
    let mut f = Facts {
        title: p.title.clone().map(|t| (t, p.confidence.title)),
        year: p.year.map(|y| (y, p.confidence.year)),
        doi: p.doi.clone(),
        arxiv_id: p.arxiv_id.clone(),
        authors: names(&p.authors),
        et_al: p.et_al || p.authors.len() > MAX_AUTHORS,
        venue: p.venue.clone().map(|v| (v, p.confidence.venue)),
        first_surname: p.first_surname(),
        ..Facts::default()
    };
    if let Some(work) = input.reference_works.get(&index) {
        let w = facts_of_work(work);
        f.title = w.title.or(f.title);
        f.year = w.year.or(f.year);
        f.doi = w.doi.or(f.doi);
        f.arxiv_id = w.arxiv_id.or(f.arxiv_id);
        f.openalex_id = w.openalex_id;
        if !w.authors.is_empty() {
            f.authors = w.authors;
            f.et_al = w.et_al;
            f.first_surname = w.first_surname;
        }
        f.venue = w.venue.or(f.venue);
        f.cited_by_count = w.cited_by_count;
    }
    Some(f)
}

fn keys(facts: &Facts, local: bool) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(d) = &facts.doi {
        out.push(format!("doi:{d}"));
    }
    if let Some(a) = &facts.arxiv_id {
        out.push(format!("arxiv:{a}"));
    }
    if let Some(o) = &facts.openalex_id {
        out.push(format!("openalex:{o}"));
    }
    if !local {
        if let Some((title, _)) = &facts.title {
            let norm = normalize_title(title);
            if norm.split(' ').count() >= 4 {
                out.push(format!("title:{norm}"));
            } else if let Some((year, _)) = facts.year {
                out.push(format!("title:{norm}|{year}"));
            }
        }
    }
    out
}

/// Whether a reference is the library paper by the local title rule.
fn same_by_title(reference: &Facts, local: &Facts) -> bool {
    let (Some((rt, _)), Some((lt, _))) = (&reference.title, &local.title) else {
        return false;
    };
    if title_similarity(rt, lt) < LOCAL_TITLE_THRESHOLD && !titles_agree(lt, rt) {
        return false;
    }
    if let (Some((ry, _)), Some((ly, _))) = (reference.year, local.year) {
        if (ry - ly).abs() > 1 {
            return false;
        }
    }
    if let (Some(rs), Some(ls)) = (&reference.first_surname, &local.first_surname) {
        if rs != ls {
            return false;
        }
    }
    true
}

/// The author entity id of a printed name: `author:<surname>-<first initial>`.
pub fn author_id(name: &str) -> Option<String> {
    let parts: Vec<&str> = name.split_whitespace().collect();
    let surname = super::text::normalize_surname(parts.last()?);
    if surname.is_empty() {
        return None;
    }
    let initial = parts
        .first()
        .filter(|_| parts.len() > 1)
        .and_then(|g| {
            super::text::fold_diacritics(g)
                .chars()
                .find(|c| c.is_alphabetic())
        })
        .map(|c| c.to_lowercase().to_string())
        .unwrap_or_default();
    Some(if initial.is_empty() {
        format!("author:{surname}")
    } else {
        format!("author:{surname}-{initial}")
    })
}

/// The venue entity id of a venue name.
pub fn venue_id(name: &str) -> Option<String> {
    let s = slug(name, 12);
    (!s.is_empty()).then(|| format!("venue:{s}"))
}

fn node_id(facts: &Facts) -> String {
    if let Some(a) = &facts.arxiv_id {
        return format!("paper:arxiv:{a}");
    }
    if let Some(d) = &facts.doi {
        return format!("paper:doi:{d}");
    }
    if let Some(o) = &facts.openalex_id {
        return format!("paper:openalex:{o}");
    }
    let basis = facts
        .title
        .as_ref()
        .map(|(t, _)| normalize_title(t))
        .unwrap_or_default();
    let year = facts.year.map(|(y, _)| y.to_string()).unwrap_or_default();
    let digest = Sha256::digest(format!("{basis}|{year}").as_bytes());
    format!("paper:title:{}", hex::encode(&digest[..8]))
}

/// The best of several weighted values: highest weight, then the most frequent, then the
/// first in order.
fn best<T: Clone + Ord>(values: impl Iterator<Item = (T, f64)>) -> Option<T> {
    let mut score: BTreeMap<T, (f64, usize, usize)> = BTreeMap::new();
    for (order, (value, weight)) in values.enumerate() {
        let entry = score.entry(value).or_insert((0.0, 0, order));
        entry.0 = entry.0.max(weight);
        entry.1 += 1;
    }
    score
        .into_iter()
        .max_by(|a, b| {
            a.1 .0
                .total_cmp(&b.1 .0)
                .then(a.1 .1.cmp(&b.1 .1))
                .then(b.1 .2.cmp(&a.1 .2))
        })
        .map(|(v, _)| v)
}

static NEW_STYLE_ARXIV: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[0-9]{4}\.[0-9]{4,5}$").ok());
static DOI_SHAPE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^10\.[0-9]{4,9}/\S+$").ok());
static PROPOSES: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)\bwe\s+(?:propose|introduce|present|develop|call)\b([^.]{0,160})").ok()
});

fn shaped(re: &LazyLock<Option<Regex>>, text: &str) -> bool {
    re.as_ref().is_some_and(|r| r.is_match(text))
}

/// Labels too generic to match in headings, captions or abstracts.
const GENERIC: &[&str] = &[
    "ours", "our", "model", "method", "baseline", "base", "large", "small", "all", "average",
    "avg", "mean", "total", "test", "train", "dev", "full", "none", "standard", "results",
];

fn phrase_in(text_norm: &str, label: &str) -> bool {
    let l = normalize_title(label);
    if l.chars().count() < 3 || GENERIC.contains(&l.as_str()) {
        return false;
    }
    format!(" {text_norm} ").contains(&format!(" {l} "))
}

struct Cluster {
    mentions: Vec<Mention>,
    facts: Vec<Facts>,
}

/// Assembles the graph from the scans of the library's PDFs.
pub fn assemble(inputs: &[ScanInput]) -> Assembled {
    let mut mentions: Vec<Mention> = Vec::new();
    let mut facts: Vec<Facts> = Vec::new();
    let mut unidentified = 0usize;
    for (f, input) in inputs.iter().enumerate() {
        mentions.push(Mention::Local(f));
        facts.push(facts_of_local(input));
        for reference in &input.scan.references {
            let identifiable = reference.parsed.is_identifiable()
                || input.reference_works.contains_key(&reference.index);
            match facts_of_reference(input, reference.index).filter(|_| identifiable) {
                Some(fr) => {
                    mentions.push(Mention::Reference(f, reference.index));
                    facts.push(fr);
                }
                None => unidentified += 1,
            }
        }
    }

    let mut uf = UnionFind((0..mentions.len()).collect());
    let mut owner: HashMap<String, usize> = HashMap::new();
    for (i, fact) in facts.iter().enumerate() {
        let local = matches!(mentions[i], Mention::Local(_));
        for key in keys(fact, local) {
            match owner.get(&key) {
                Some(&j) => uf.union(i, j),
                None => {
                    owner.insert(key, i);
                }
            }
        }
    }
    let locals: Vec<usize> = (0..mentions.len())
        .filter(|&i| matches!(mentions[i], Mention::Local(_)))
        .collect();
    let mut local_title_links = 0;
    for i in 0..mentions.len() {
        if !matches!(mentions[i], Mention::Reference(..)) {
            continue;
        }
        let root = uf.find(i);
        if locals.iter().any(|&l| uf.find(l) == root) {
            continue;
        }
        let matches: Vec<usize> = locals
            .iter()
            .copied()
            .filter(|&l| same_by_title(&facts[i], &facts[l]))
            .collect();
        if let [only] = matches.as_slice() {
            uf.union(i, *only);
            local_title_links += 1;
        }
    }

    // Clusters, in the order of their first mention.
    let mut cluster_of: BTreeMap<usize, usize> = BTreeMap::new();
    let mut clusters: Vec<Cluster> = Vec::new();
    for i in 0..mentions.len() {
        let root = uf.find(i);
        let c = *cluster_of.entry(root).or_insert_with(|| {
            clusters.push(Cluster {
                mentions: Vec::new(),
                facts: Vec::new(),
            });
            clusters.len() - 1
        });
        clusters[c].mentions.push(mentions[i]);
        clusters[c].facts.push(facts[i].clone());
    }
    let mut mention_cluster: HashMap<Mention, usize> = HashMap::new();
    for (c, cluster) in clusters.iter().enumerate() {
        for m in &cluster.mentions {
            mention_cluster.insert(*m, c);
        }
    }

    // Nodes.
    let mut papers: Vec<PaperNode> = Vec::new();
    let mut sources: Vec<(String, Option<u32>, f64)> = Vec::new();
    let mut author_names: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut venues: BTreeMap<String, (String, String)> = BTreeMap::new();
    for cluster in &clusters {
        let local_files: Vec<usize> = cluster
            .mentions
            .iter()
            .filter_map(|m| match m {
                Mention::Local(f) => Some(*f),
                _ => None,
            })
            .collect();
        let in_library = !local_files.is_empty();
        let local_file = local_files
            .iter()
            .copied()
            .min_by(|a, b| inputs[*a].scan.file_path.cmp(&inputs[*b].scan.file_path));
        // Library facts first: a library paper's own identity outranks how others cite it.
        let ordered: Vec<&Facts> = cluster
            .mentions
            .iter()
            .zip(&cluster.facts)
            .filter(|(m, _)| matches!(m, Mention::Local(_)))
            .chain(
                cluster
                    .mentions
                    .iter()
                    .zip(&cluster.facts)
                    .filter(|(m, _)| !matches!(m, Mention::Local(_))),
            )
            .map(|(_, f)| f)
            .collect();
        let merged = Facts {
            title: ordered.iter().find_map(|f| f.title.clone()).map(|(t, w)| {
                let pick = if in_library {
                    t.clone()
                } else {
                    best(ordered.iter().filter_map(|f| f.title.clone())).unwrap_or(t)
                };
                (pick, w)
            }),
            year: if in_library {
                ordered.iter().find_map(|f| f.year)
            } else {
                best(ordered.iter().filter_map(|f| f.year)).map(|y| (y, 0.9))
            },
            doi: ordered.iter().find_map(|f| f.doi.clone()),
            arxiv_id: ordered.iter().find_map(|f| f.arxiv_id.clone()),
            openalex_id: ordered.iter().find_map(|f| f.openalex_id.clone()),
            authors: ordered
                .iter()
                .find(|f| f.openalex_id.is_some() && !f.authors.is_empty())
                .or_else(|| ordered.iter().max_by_key(|f| f.authors.len()))
                .map(|f| f.authors.clone())
                .unwrap_or_default(),
            et_al: ordered.iter().any(|f| f.et_al),
            venue: ordered
                .iter()
                .filter_map(|f| f.venue.clone())
                .max_by(|a, b| a.1.total_cmp(&b.1)),
            cited_by_count: ordered.iter().find_map(|f| f.cited_by_count),
            first_surname: None,
        };
        let id = node_id(&merged);
        let author_ids: Vec<String> = merged.authors.iter().filter_map(|a| author_id(a)).collect();
        if in_library {
            for (name, aid) in merged
                .authors
                .iter()
                .zip(merged.authors.iter().map(|a| author_id(a)))
            {
                if let Some(aid) = aid {
                    let file = local_file
                        .map(|f| inputs[f].scan.file_path.clone())
                        .unwrap_or_default();
                    let entry = author_names.entry(aid).or_insert((name.clone(), file));
                    if name.chars().count() > entry.0.chars().count() {
                        entry.0 = name.clone();
                    }
                }
            }
        }
        let venue = merged.venue.as_ref().map(|(v, _)| v.clone());
        // Provenance: the library file itself, or the first library file citing the work.
        let (source, page, confidence) = match local_file {
            Some(f) => (inputs[f].scan.file_path.clone(), Some(1), 0.95),
            None => {
                let citing = cluster
                    .mentions
                    .iter()
                    .filter_map(|m| match m {
                        Mention::Reference(f, r) => Some((*f, *r)),
                        _ => None,
                    })
                    .min_by(|a, b| {
                        inputs[a.0]
                            .scan
                            .file_path
                            .cmp(&inputs[b.0].scan.file_path)
                            .then(a.1.cmp(&b.1))
                    });
                match citing {
                    Some((f, r)) => {
                        let Some(reference) =
                            inputs[f].scan.references.iter().find(|x| x.index == r)
                        else {
                            continue;
                        };
                        let confidence = if inputs[f].reference_works.contains_key(&r) {
                            0.95
                        } else {
                            reference.parsed.confidence_overall()
                        };
                        (inputs[f].scan.file_path.clone(), reference.page, confidence)
                    }
                    None => (String::new(), None, 0.5),
                }
            }
        };
        if let (Some(v), Some(vid)) = (&venue, venue.as_deref().and_then(venue_id)) {
            venues
                .entry(vid)
                .or_insert_with(|| (v.clone(), source.clone()));
        }
        let mut authors_printed = merged.authors.join(", ");
        if merged.et_al && !authors_printed.is_empty() {
            authors_printed.push_str(" et al.");
        }
        papers.push(PaperNode {
            id,
            title: merged.title.map(|(t, _)| t),
            year: merged.year.map(|(y, _)| y),
            doi: merged.doi,
            arxiv_id: merged.arxiv_id,
            openalex_id: merged.openalex_id,
            authors: (!authors_printed.is_empty()).then_some(authors_printed),
            author_ids,
            venue,
            in_library,
            file_path: local_file.map(|f| inputs[f].scan.file_path.clone()),
            cited_by_count: merged.cited_by_count,
            statement_id: None,
        });
        sources.push((source, page, confidence));
    }

    // Two clusters can still derive the same id (a title hash); keep ids unique by
    // merging nothing further but suffixing the later one.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for paper in papers.iter_mut() {
        let n = seen.entry(paper.id.clone()).or_insert(0);
        if *n > 0 {
            paper.id = format!("{}-{n}", paper.id);
        }
        *n += 1;
    }

    // Citations.
    let mut cites: Vec<(String, String, Option<CitationEvidence>)> = Vec::new();
    let mut cite_lists: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (f, input) in inputs.iter().enumerate() {
        let Some(&from) = mention_cluster.get(&Mention::Local(f)) else {
            continue;
        };
        for reference in &input.scan.references {
            let Some(&to) = mention_cluster.get(&Mention::Reference(f, reference.index)) else {
                continue;
            };
            if to == from || !cite_lists.entry(from).or_default().insert(to) {
                continue;
            }
            cites.push((
                papers[from].id.clone(),
                papers[to].id.clone(),
                Some(CitationEvidence {
                    text: reference.parsed.text.clone(),
                    page: reference.page,
                    regions: reference.regions.clone(),
                }),
            ));
        }
    }

    // Methods and datasets.
    let mut methods: BTreeMap<String, String> = BTreeMap::new();
    let mut datasets: BTreeMap<String, String> = BTreeMap::new();
    for input in inputs {
        for (id, label) in &input.result_methods {
            methods.entry(id.clone()).or_insert_with(|| label.clone());
        }
        for (id, label) in &input.result_datasets {
            datasets.entry(id.clone()).or_insert_with(|| label.clone());
        }
    }
    let mut uses: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    let mut evaluated: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for (f, input) in inputs.iter().enumerate() {
        let Some(&c) = mention_cluster.get(&Mention::Local(f)) else {
            continue;
        };
        let u = uses.entry(c).or_default();
        u.extend(input.result_methods.iter().map(|(id, _)| id.clone()));
        let headings = normalize_title(&input.scan.headings.join(" . "));
        for (id, label) in &methods {
            if phrase_in(&headings, label) {
                u.insert(id.clone());
            }
        }
        let e = evaluated.entry(c).or_default();
        e.extend(input.result_datasets.iter().map(|(id, _)| id.clone()));
        let captions = normalize_title(&input.scan.captions.join(" . "));
        for (id, label) in &datasets {
            if phrase_in(&captions, label) {
                e.insert(id.clone());
            }
        }
    }
    let mut proposed: BTreeMap<String, usize> = BTreeMap::new();
    for (id, label) in &methods {
        let claims: Vec<usize> = inputs
            .iter()
            .enumerate()
            .filter(|(_, input)| proposes(input, label))
            .filter_map(|(f, _)| mention_cluster.get(&Mention::Local(f)).copied())
            .collect::<BTreeSet<usize>>()
            .into_iter()
            .collect();
        let earliest = claims
            .iter()
            .filter_map(|&c| papers[c].year.map(|y| (y, c)))
            .min();
        let chosen = match (claims.as_slice(), earliest) {
            ([only], _) => Some(*only),
            (_, Some((year, c)))
                if claims
                    .iter()
                    .filter(|&&o| papers[o].year.is_some_and(|y| y <= year))
                    .count()
                    == 1 =>
            {
                Some(c)
            }
            _ => None,
        };
        if let Some(c) = chosen {
            proposed.insert(id.clone(), c);
        }
    }

    // Statements.
    let mut statements = Vec::new();
    for (c, paper) in papers.iter().enumerate() {
        let mut p: BTreeMap<String, RawValue> = BTreeMap::new();
        if let Some(t) = &paper.title {
            p.insert("title".into(), RawValue::text(t.clone()));
        }
        if let Some(y) = paper.year {
            p.insert("publicationYear".into(), RawValue::Integer(i64::from(y)));
        }
        if let Some(d) = paper.doi.as_deref().filter(|d| shaped(&DOI_SHAPE, d)) {
            p.insert("doi".into(), RawValue::text(d));
        }
        if let Some(a) = paper
            .arxiv_id
            .as_deref()
            .filter(|a| shaped(&NEW_STYLE_ARXIV, a))
        {
            p.insert("arxivId".into(), RawValue::text(a));
        }
        if let Some(o) = &paper.openalex_id {
            p.insert("openalexId".into(), RawValue::text(o.clone()));
        }
        if let Some(a) = &paper.authors {
            p.insert("authorsAsPrinted".into(), RawValue::text(a.clone()));
        }
        if !paper.author_ids.is_empty() {
            let mut unique: Vec<String> = Vec::new();
            for a in &paper.author_ids {
                if !unique.contains(a) {
                    unique.push(a.clone());
                }
            }
            p.insert(
                "authoredBy".into(),
                RawValue::List(
                    unique
                        .into_iter()
                        .map(|a| RawValue::Entity(EntityRef::typed(a, "Author")))
                        .collect(),
                ),
            );
        }
        if let Some(vid) = paper.venue.as_deref().and_then(venue_id) {
            p.insert(
                "publishedIn".into(),
                RawValue::Entity(EntityRef::typed(vid, "Venue")),
            );
        }
        if paper.in_library {
            p.insert("inLibrary".into(), RawValue::Boolean(true));
        }
        if let Some(n) = paper.cited_by_count {
            p.insert("citedByCount".into(), RawValue::Integer(n));
        }
        let entity_list = |ids: Vec<String>, class: &str| {
            RawValue::List(
                ids.into_iter()
                    .map(|id| RawValue::Entity(EntityRef::typed(id, class)))
                    .collect(),
            )
        };
        if let Some(targets) = cite_lists.get(&c).filter(|t| !t.is_empty()) {
            let mut ids: Vec<String> = targets.iter().map(|&t| papers[t].id.clone()).collect();
            ids.sort();
            p.insert("cites".into(), entity_list(ids, "Paper"));
        }
        if let Some(m) = uses.get(&c).filter(|m| !m.is_empty()) {
            p.insert(
                "usesMethod".into(),
                entity_list(m.iter().cloned().collect(), "Method"),
            );
        }
        if let Some(d) = evaluated.get(&c).filter(|d| !d.is_empty()) {
            p.insert(
                "evaluatedOn".into(),
                entity_list(d.iter().cloned().collect(), "Dataset"),
            );
        }
        let (source, page, confidence) = sources[c].clone();
        if source.is_empty() {
            continue;
        }
        statements.push(DesiredStatement {
            class: "Paper",
            subject: paper.id.clone(),
            properties: p,
            source,
            page,
            confidence,
        });
    }
    for (aid, (name, file)) in &author_names {
        statements.push(DesiredStatement {
            class: "Author",
            subject: aid.clone(),
            properties: BTreeMap::from([("name".to_string(), RawValue::text(name.clone()))]),
            source: file.clone(),
            page: Some(1),
            confidence: 0.8,
        });
    }
    for (vid, (name, source)) in &venues {
        if source.is_empty() {
            continue;
        }
        statements.push(DesiredStatement {
            class: "Venue",
            subject: vid.clone(),
            properties: BTreeMap::from([("name".to_string(), RawValue::text(name.clone()))]),
            source: source.clone(),
            page: None,
            confidence: 0.8,
        });
    }
    for (mid, &c) in &proposed {
        let Some(source) = papers[c].file_path.clone() else {
            continue;
        };
        statements.push(DesiredStatement {
            class: "Method",
            subject: mid.clone(),
            properties: BTreeMap::from([
                (
                    "name".to_string(),
                    RawValue::text(methods.get(mid).cloned().unwrap_or_else(|| mid.clone())),
                ),
                (
                    "proposedIn".to_string(),
                    RawValue::Entity(EntityRef::typed(papers[c].id.clone(), "Paper")),
                ),
            ]),
            source,
            page: Some(1),
            confidence: 0.7,
        });
    }

    let parts = GraphParts {
        uses_method: uses
            .iter()
            .flat_map(|(&c, ms)| ms.iter().map(move |m| (c, m.clone())))
            .map(|(c, m)| (papers[c].id.clone(), m))
            .collect(),
        evaluated_on: evaluated
            .iter()
            .flat_map(|(&c, ds)| ds.iter().map(move |d| (c, d.clone())))
            .map(|(c, d)| (papers[c].id.clone(), d))
            .collect(),
        proposed_in: proposed
            .iter()
            .map(|(m, &c)| (m.clone(), papers[c].id.clone()))
            .collect(),
        methods: methods
            .into_iter()
            .map(|(id, label)| ConceptNode { id, label })
            .collect(),
        datasets: datasets
            .into_iter()
            .map(|(id, label)| ConceptNode { id, label })
            .collect(),
        authors: author_names
            .into_iter()
            .map(|(id, (name, _))| (id, name))
            .collect(),
        cites,
        papers,
    };
    Assembled {
        statements,
        parts,
        unidentified,
        local_title_links,
    }
}

/// Whether a library paper proposes the method `label` (title short name or abstract).
fn proposes(input: &ScanInput, label: &str) -> bool {
    let norm_label = normalize_title(label);
    if norm_label.chars().count() < 2 || GENERIC.contains(&norm_label.as_str()) {
        return false;
    }
    let title = input
        .scan
        .identity
        .title
        .clone()
        .or_else(|| input.work.as_ref().map(|w| w.title.clone()));
    if title
        .as_deref()
        .and_then(title_alias)
        .is_some_and(|alias| normalize_title(&alias) == norm_label)
    {
        return true;
    }
    let Some(abstract_text) = &input.scan.identity.abstract_text else {
        return false;
    };
    PROPOSES.as_ref().is_some_and(|re| {
        re.captures_iter(abstract_text).any(|c| {
            c.get(1)
                .is_some_and(|tail| phrase_in(&normalize_title(tail.as_str()), label))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research::citations::identity::LocalIdentity;
    use crate::research::citations::reference::parse_reference;
    use crate::research::citations::scan::{ScannedReference, SCAN_VERSION};

    fn scan(path: &str, title: &str, arxiv: Option<&str>, refs: &[&str]) -> PaperScan {
        PaperScan {
            version: SCAN_VERSION.into(),
            file_path: path.into(),
            identity: LocalIdentity {
                title: Some(title.into()),
                arxiv_id: arxiv.map(str::to_string),
                year: arxiv.and_then(super::super::identity::arxiv_year),
                ..LocalIdentity::default()
            },
            references: refs
                .iter()
                .enumerate()
                .map(|(index, text)| ScannedReference {
                    index,
                    page: Some(10),
                    regions: Vec::new(),
                    parsed: parse_reference(text),
                })
                .collect(),
            rejected: Vec::new(),
            truncated: 0,
            headings: Vec::new(),
            captions: Vec::new(),
            pages: 12,
        }
    }

    fn input(scan: PaperScan) -> ScanInput {
        ScanInput {
            scan,
            work: None,
            reference_works: HashMap::new(),
            result_methods: Vec::new(),
            result_datasets: Vec::new(),
        }
    }

    const FWP_REF: &str = "Imanol Schlag, Kazuki Irie, and Jürgen Schmidhuber. 2021. Linear transformers are secretly fast weight programmers. In International Conference on Machine Learning, pages 9355–9366. PMLR.";
    const ATTENTION_ACL: &str = "Ashish Vaswani, Noam Shazeer, Niki Parmar, Jakob Uszkoreit, Llion Jones, Aidan N Gomez, Łukasz Kaiser, and Illia Polosukhin. 2017. Attention is all you need. In Advances in Neural Information Processing Systems.";
    const ATTENTION_ICML: &str = "Vaswani, A., Shazeer, N., Parmar, N., Uszkoreit, J., Jones, L., Gomez, A. N., Kaiser, L., and Polosukhin, I. Attention is all you need. In NeurIPS, 2017.";

    #[test]
    fn mentions_of_one_work_become_one_node_and_references_link_to_library_papers() {
        let fwp = scan(
            "C:/p/fwp.pdf",
            "Linear Transformers Are Secretly Fast Weight Programmers",
            None,
            &[ATTENTION_ICML],
        );
        let delta = scan(
            "C:/p/delta.pdf",
            "Parallelizing Linear Transformers with the Delta Rule over Sequence Length",
            Some("2406.06484"),
            &[FWP_REF, ATTENTION_ACL, "pages 1–10."],
        );
        let built = assemble(&[input(fwp), input(delta)]);
        let papers = &built.parts.papers;
        assert_eq!(papers.iter().filter(|p| p.in_library).count(), 2);
        let attention: Vec<&PaperNode> = papers
            .iter()
            .filter(|p| {
                p.title
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case("Attention is all you need"))
            })
            .collect();
        assert_eq!(attention.len(), 1, "one node for both spellings");
        assert_eq!(
            built.local_title_links, 1,
            "the FWP reference is the library paper"
        );
        assert_eq!(built.unidentified, 1);
        let delta_node = papers
            .iter()
            .find(|p| p.id == "paper:arxiv:2406.06484")
            .unwrap();
        let fwp_node = papers
            .iter()
            .find(|p| p.file_path.as_deref() == Some("C:/p/fwp.pdf"))
            .unwrap();
        assert!(built
            .parts
            .cites
            .iter()
            .any(|(a, b, e)| a == &delta_node.id && b == &fwp_node.id && e.is_some()));
        // Statements: one per paper node, library papers flagged.
        let paper_statements: Vec<&DesiredStatement> = built
            .statements
            .iter()
            .filter(|s| s.class == "Paper")
            .collect();
        assert_eq!(paper_statements.len(), papers.len());
        let delta_statement = paper_statements
            .iter()
            .find(|s| s.subject == delta_node.id)
            .unwrap();
        assert_eq!(
            delta_statement.properties.get("inLibrary"),
            Some(&RawValue::Boolean(true))
        );
        assert_eq!(delta_statement.source, "C:/p/delta.pdf");
        match delta_statement.properties.get("cites") {
            Some(RawValue::List(list)) => assert_eq!(list.len(), 2),
            other => panic!("{other:?}"),
        }
        // The same inputs give the same statements.
        let again = assemble(&[
            input(scan(
                "C:/p/fwp.pdf",
                "Linear Transformers Are Secretly Fast Weight Programmers",
                None,
                &[ATTENTION_ICML],
            )),
            input(scan(
                "C:/p/delta.pdf",
                "Parallelizing Linear Transformers with the Delta Rule over Sequence Length",
                Some("2406.06484"),
                &[FWP_REF, ATTENTION_ACL, "pages 1–10."],
            )),
        ]);
        assert_eq!(again.statements, built.statements);
    }

    #[test]
    fn openalex_matches_add_identity_and_counts() {
        let mut delta = input(scan(
            "C:/p/delta.pdf",
            "Parallelizing Linear Transformers with the Delta Rule over Sequence Length",
            Some("2406.06484"),
            &[FWP_REF],
        ));
        delta.reference_works.insert(
            0,
            OpenAlexWork {
                id: "W3166702123".into(),
                doi: Some("10.48550/arxiv.2102.11174".into()),
                arxiv_id: Some("2102.11174".into()),
                title: "Linear Transformers Are Secretly Fast Weight Programmers".into(),
                year: Some(2021),
                authors: vec![
                    "Imanol Schlag".into(),
                    "Kazuki Irie".into(),
                    "Jürgen Schmidhuber".into(),
                ],
                venue: Some("arXiv (Cornell University)".into()),
                cited_by_count: Some(23),
            },
        );
        let built = assemble(&[delta]);
        let fwp = built
            .parts
            .papers
            .iter()
            .find(|p| p.id == "paper:arxiv:2102.11174")
            .unwrap();
        assert_eq!(fwp.openalex_id.as_deref(), Some("W3166702123"));
        assert_eq!(fwp.cited_by_count, Some(23));
        assert_eq!(fwp.author_ids[0], "author:schlag-i");
        assert!(!fwp.in_library);
        assert!(built.statements.iter().any(|s| s.class == "Venue"));
    }

    #[test]
    fn methods_come_from_results_headings_and_captions_and_proposals_from_titles() {
        let mut kan = input(scan(
            "C:/p/kan.pdf",
            "KAN: Kolmogorov-Arnold Networks",
            Some("2404.19756"),
            &[],
        ));
        kan.result_methods = vec![
            ("method:kan".into(), "KAN".into()),
            ("method:mlp".into(), "MLP".into()),
        ];
        kan.result_datasets = vec![("dataset:feynman".into(), "Feynman".into())];
        let mut other = input(scan(
            "C:/p/other.pdf",
            "Some later study of networks",
            Some("2405.00001"),
            &[],
        ));
        other.scan.headings = vec!["4 Comparison with KAN".into()];
        other.scan.captions = vec!["Table 2: Error on the Feynman dataset.".into()];
        let built = assemble(&[kan, other]);
        let other_id = "paper:arxiv:2405.00001".to_string();
        assert!(built
            .parts
            .uses_method
            .contains(&(other_id.clone(), "method:kan".into())));
        assert!(built
            .parts
            .evaluated_on
            .contains(&(other_id, "dataset:feynman".into())));
        assert_eq!(
            built.parts.proposed_in,
            vec![(
                "method:kan".to_string(),
                "paper:arxiv:2404.19756".to_string()
            )]
        );
        assert!(built
            .statements
            .iter()
            .any(|s| s.class == "Method" && s.subject == "method:kan"));
        assert_eq!(
            author_id("Jürgen Schmidhuber").as_deref(),
            Some("author:schmidhuber-j")
        );
        assert_eq!(author_id("OpenAI").as_deref(), Some("author:openai"));
        assert_eq!(venue_id("NeurIPS").as_deref(), Some("venue:neurips"));
    }
}
