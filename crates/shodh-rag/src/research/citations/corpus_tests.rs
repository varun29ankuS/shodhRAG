//! The reference parser on real bibliographies: the layout parser's bibliography blocks
//! of seven public arXiv papers (`fixtures/corpus_references.json`, built by
//! [`super::corpus_dump`]), segmented and parsed, with a hand-labelled sample across the
//! styles they use (ICML/NeurIPS author–title–year, numbered NeurIPS, ACL-like author-first,
//! quoted titles) and floors on how much of each bibliography is read.

use serde_json::Value;

use super::reference::{parse_reference, ParsedReference};
use super::segment::{segment, BibBlock};

const FIXTURE: &str = include_str!("fixtures/corpus_references.json");

fn papers() -> Vec<(String, Vec<ParsedReference>, usize)> {
    let all: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut out = Vec::new();
    for (name, paper) in all.as_object().unwrap() {
        let blocks: Vec<BibBlock> = paper["references"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| BibBlock {
                text: r["text"].as_str().unwrap().to_string(),
                page: r["page"].as_u64().map(|p| p as u32),
                bbox: None,
                section_path: r["section"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect(),
            })
            .collect();
        let s = segment(&blocks);
        let parsed = s.entries.iter().map(|e| parse_reference(&e.text)).collect();
        out.push((name.clone(), parsed, s.rejected.len()));
    }
    out
}

fn find<'a>(refs: &'a [ParsedReference], needle: &str) -> &'a ParsedReference {
    refs.iter()
        .find(|r| r.text.contains(needle))
        .unwrap_or_else(|| panic!("no entry contains {needle:?}"))
}

/// `(paper prefix, text in the entry, title, year, first surname, arXiv id)`.
const LABELLED: &[(&str, &str, &str, i32, &str, Option<&str>)] = &[
    ("2102.11174", "Ba, J., Hinton", "Using fast weights to attend to the recent past", 2016, "ba", None),
    ("2102.11174", "Baevski, A. and Auli", "Adaptive input representations for neural language modeling", 2019, "baevski", None),
    ("2406.06484", "Akyürek", "In-context language learning: Arhitectures and algorithms", 2024, "akyurek", Some("2401.12973")),
    ("2406.06484", "Aksenov", "Linear Transformers with Learnable Kernel Functions are Better In-Context Models", 2024, "aksenov", Some("2402.10644")),
    ("2406.06484", "The hidden attention of mamba", "The hidden attention of mamba models", 2024, "ali", None),
    ("2411.12537", "William Merrill, Jackson Petty", "The Illusion of State in State-Space Models", 2024, "merrill", None),
    ("2411.12537", "Satwik Bhattamishra", "On the ability and limitations of transformers to recognize formal languages", 2020, "bhattamishra", None),
    ("2505.01730", "Stochastic gradient descent tricks", "Stochastic gradient descent tricks", 2012, "bottou", None),
    ("2505.01730", "Optimal ann-snn conversion", "Optimal ann-snn conversion for high-accuracy and ultra-low-latency spiking neural networks", 2023, "bu", Some("2303.04347")),
    ("2505.01730", "Spatio-temporal backpropagation", "Spatio-temporal backpropagation for training high-performance spiking neural networks", 2018, "wu", None),
    ("2407.04620", "Gpt-4 technical report", "Gpt-4 technical report", 2023, "achiam", Some("2303.08774")),
    ("2407.04620", "Self-supervised policy adaptation", "Self-supervised policy adaptation during deployment", 2020, "hansen", Some("2007.04309")),
    ("2504.13173", "Unsupervised Representation Learning of Brain Activity", "Unsupervised Representation Learning of Brain Activity via Bridging Voxel Activity and Functional Connectivity", 2024, "behrouz", None),
    ("2504.13173", "Scaling laws for neural language models", "Scaling laws for neural language models", 2020, "kaplan", Some("2001.08361")),
    ("2605.04308", "Gpt-4 technical report", "Gpt-4 technical report", 2023, "achiam", Some("2303.08774")),
    ("2605.04308", "From self-attention to markov models", "From self-attention to markov models: Unveiling the dynamics of generative transformers", 2024, "ildiz", Some("2402.13512")),
];

#[test]
fn the_hand_labelled_sample_parses_field_by_field() {
    let papers = papers();
    let mut wrong = Vec::new();
    for (paper, needle, title, year, surname, arxiv) in LABELLED {
        let (_, refs, _) = papers
            .iter()
            .find(|(name, _, _)| name.starts_with(paper))
            .unwrap();
        let r = find(refs, needle);
        if r.title.as_deref() != Some(*title) {
            wrong.push(format!("{paper} {needle}: title {:?}", r.title));
        }
        if r.year != Some(*year) {
            wrong.push(format!("{paper} {needle}: year {:?}", r.year));
        }
        if r.first_surname().as_deref() != Some(*surname) {
            wrong.push(format!(
                "{paper} {needle}: first surname {:?}",
                r.first_surname()
            ));
        }
        if r.arxiv_id.as_deref() != *arxiv {
            wrong.push(format!("{paper} {needle}: arXiv {:?}", r.arxiv_id));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} fields wrong:\n{}",
        wrong.len(),
        LABELLED.len() * 4,
        wrong.join("\n")
    );
}

#[test]
fn most_of_every_bibliography_is_read() {
    let mut total = 0;
    let mut identifiable = 0;
    let mut with_title_year = 0;
    let mut with_authors = 0;
    for (name, refs, rejected) in papers() {
        let n = refs.len();
        let id = refs.iter().filter(|r| r.is_identifiable()).count();
        let ty = refs
            .iter()
            .filter(|r| r.title.is_some() && r.year.is_some())
            .count();
        let au = refs.iter().filter(|r| !r.authors.is_empty()).count();
        println!("{name}: {n} entries, {rejected} rejected blocks, {id} identifiable, {ty} with title and year, {au} with authors");
        assert!(n >= 30, "{name}: only {n} entries");
        assert!(id * 100 >= n * 90, "{name}: {id} of {n} identifiable");
        total += n;
        identifiable += id;
        with_title_year += ty;
        with_authors += au;
    }
    println!("all: {total} entries, {identifiable} identifiable, {with_title_year} with title and year, {with_authors} with authors");
    assert!(
        with_title_year * 100 >= total * 90,
        "{with_title_year} of {total} with title and year"
    );
    assert!(
        with_authors * 100 >= total * 90,
        "{with_authors} of {total} with authors"
    );
}
