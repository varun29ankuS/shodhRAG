//! Header-aware interpretation of a parsed table into Result candidates.
//!
//! Deterministic first:
//! - leading body rows without any number continue the header (multi-row headers: a
//!   dataset spanning several metric columns, units, arrows);
//! - the leading columns whose body cells are mostly words hold the method (row labels);
//!   a body row with a label and no numbers is a group heading (`340M params`) and becomes
//!   part of the setting of the rows under it;
//! - each value column's header text names a metric (from a lexicon of evaluation measures)
//!   and, in what remains, a dataset; size or shot qualifiers (`2K`, `5-shot`, `340M`) are
//!   the setting; a dataset or metric not in the header may come from the caption when the
//!   caption names exactly one;
//! - a value is read only from a cell holding exactly one number ([`super::numbers`]).
//!
//! A table whose rows hold datasets with the metric in parentheses (`MMLU-Pro (EM)`) and
//! whose columns are methods is read transposed.
//!
//! When a column's metric or dataset cannot be named this way, a language model may be
//! asked to name them ([`HeaderRoles`]); its labels are accepted only when they occur
//! verbatim in the table's header, labels or caption, and values are never taken from it.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::numbers::{header_unit, read_cell, CellIssue, CellNumber};
use crate::processing::document_model::BBox;

/// A parsed table block as the interpreter needs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInput {
    pub page: u32,
    pub bbox: Option<BBox>,
    pub caption: Option<String>,
    /// A `Table N` caption found near the table on the same page when the parser did not
    /// attach one (weaker evidence).
    #[serde(default)]
    pub nearby_caption: Option<String>,
    #[serde(default)]
    pub section_path: Vec<String>,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Header row first, aligned with `header` and `rows`.
    #[serde(default)]
    pub cell_boxes: Vec<Vec<Option<BBox>>>,
}

impl TableInput {
    /// The caption to read datasets and metrics from, and whether it is the parser's.
    fn caption(&self) -> Option<(&str, bool)> {
        self.caption
            .as_deref()
            .map(|c| (c, true))
            .or_else(|| self.nearby_caption.as_deref().map(|c| (c, false)))
    }

    /// `Table 3` from the caption, when it has a number.
    pub fn label(&self) -> Option<String> {
        static LABEL: LazyLock<Option<Regex>> =
            LazyLock::new(|| Regex::new(r"(?i)^\s*(table\s+[0-9]+[a-z]?)").ok());
        let (caption, _) = self.caption()?;
        LABEL
            .as_ref()?
            .captures(caption)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().split_whitespace().collect::<Vec<_>>().join(" "))
    }

    fn cell_box(&self, row: usize, column: usize) -> Option<BBox> {
        self.cell_boxes.get(row)?.get(column).copied().flatten()
    }
}

/// Where a column's dataset or metric was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// In the column's own header text.
    Header,
    /// In the parser's caption of the table.
    Caption,
    /// In a caption near the table (not attached by the parser).
    NearbyCaption,
    /// In the heading of the section the table is in.
    Section,
    /// Named by the language model (verified to occur in the table text).
    Model,
}

/// Dataset, metric and setting of one value column (or, transposed, one row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnRoles {
    pub dataset: Option<(String, Evidence)>,
    pub metric: Option<(String, Evidence)>,
    /// Qualifiers from the header that are neither (`2K`, `5-shot`).
    pub setting: Vec<String>,
    pub unit: Option<String>,
}

/// Roles a language model gave the value columns, by column index. Only labels that
/// occur in the table text are accepted ([`verify_roles`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HeaderRoles {
    #[serde(default)]
    pub columns: Vec<ModelColumn>,
}

/// One column as the model named it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelColumn {
    pub index: usize,
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub dataset: Option<String>,
    #[serde(default)]
    pub setting: Option<String>,
}

/// One value read from the table with everything a Result statement needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub method: String,
    pub dataset: String,
    pub dataset_evidence: Evidence,
    pub metric: String,
    pub metric_evidence: Evidence,
    pub number: CellNumber,
    pub cell_text: String,
    pub unit: Option<String>,
    pub setting: String,
    pub page: u32,
    /// Body row and column of the value (0-based, header rows excluded).
    pub row: usize,
    pub column: usize,
    pub cell_box: Option<BBox>,
    pub confidence: f64,
}

/// What reading one table produced.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Reading {
    pub candidates: Vec<Candidate>,
    /// Why the table gave nothing (set when `candidates` is empty).
    pub skipped: Option<String>,
    /// Value columns whose dataset or metric could not be named (candidates for the
    /// model), by column index.
    pub unresolved: Vec<usize>,
    /// Cells refused because they hold several numbers.
    pub ambiguous_cells: usize,
}

/// Confidence of a value whose dataset and metric are both in its column header.
const BASE_CONFIDENCE: f64 = 0.9;
/// Taken off when the dataset or metric came from the parser's caption (below the
/// acceptance threshold: the user reviews such values).
const CAPTION_PENALTY: f64 = 0.15;
/// Taken off when it came from a caption near the table, or from the section heading.
const NEARBY_PENALTY: f64 = 0.25;
/// Highest confidence of a value whose method is printed only as "Ours": the label
/// names no method, so the user confirms what it stands for.
const OURS_CONFIDENCE: f64 = 0.7;
/// Confidence of a value whose header roles a model named.
pub const MODEL_CONFIDENCE: f64 = 0.7;

struct MetricTerm {
    canonical: &'static str,
    pattern: &'static str,
}

/// Evaluation measures, most specific first. `{k}` patterns keep the cut-off.
const METRICS: &[MetricTerm] = &[
    MetricTerm {
        canonical: "recall@{k}",
        pattern: r"(?:recall|r)\s*@\s*(\d+)",
    },
    MetricTerm {
        canonical: "precision@{k}",
        pattern: r"(?:precision|p)\s*@\s*(\d+)",
    },
    MetricTerm {
        canonical: "ndcg@{k}",
        pattern: r"n\s*dcg\s*@\s*(\d+)",
    },
    MetricTerm {
        canonical: "mrr@{k}",
        pattern: r"mrr\s*@\s*(\d+)",
    },
    MetricTerm {
        canonical: "map@{k}",
        pattern: r"map\s*@\s*(\d+)",
    },
    MetricTerm {
        canonical: "hits@{k}",
        pattern: r"hits?\s*@\s*(\d+)",
    },
    MetricTerm {
        canonical: "pass@{k}",
        pattern: r"pass\s*@\s*(\d+)",
    },
    MetricTerm {
        canonical: "top-{k}-accuracy",
        pattern: r"top\s*-?\s*(\d+)(?:\s*acc(?:uracy)?\.?)?",
    },
    MetricTerm {
        canonical: "exact-match",
        pattern: r"exact\s+match|\bem\b",
    },
    MetricTerm {
        canonical: "rouge-{k}",
        pattern: r"rouge\s*-?\s*([12l])\b",
    },
    MetricTerm {
        canonical: "rouge",
        pattern: r"rouge",
    },
    MetricTerm {
        canonical: "bleu",
        pattern: r"(?:sacre)?bleu(?:-\d)?",
    },
    MetricTerm {
        canonical: "meteor",
        pattern: r"meteor",
    },
    MetricTerm {
        canonical: "cider",
        pattern: r"cider",
    },
    MetricTerm {
        canonical: "f1",
        pattern: r"(?:macro|micro)?\s*-?\s*f\s*-?\s*1(?:\s*-?\s*score)?|f-?score|f-?measure",
    },
    MetricTerm {
        canonical: "accuracy",
        pattern: r"acc(?:uracy)?\.?(?:(?:\s*|_)n\b)?",
    },
    MetricTerm {
        canonical: "precision",
        pattern: r"precision|prec\.",
    },
    MetricTerm {
        canonical: "recall",
        pattern: r"recall",
    },
    MetricTerm {
        canonical: "ndcg",
        pattern: r"n\s*dcg",
    },
    MetricTerm {
        canonical: "mrr",
        pattern: r"mrr",
    },
    MetricTerm {
        canonical: "map",
        pattern: r"\bmap\b|mean\s+average\s+precision",
    },
    MetricTerm {
        canonical: "perplexity",
        pattern: r"perplexity|ppl\.?",
    },
    MetricTerm {
        canonical: "auroc",
        pattern: r"au-?roc|roc-?auc",
    },
    MetricTerm {
        canonical: "auc",
        pattern: r"\bauc\b",
    },
    MetricTerm {
        canonical: "word-error-rate",
        pattern: r"\bwer\b|word\s+error\s+rate",
    },
    MetricTerm {
        canonical: "character-error-rate",
        pattern: r"\bcer\b",
    },
    MetricTerm {
        canonical: "rmse",
        pattern: r"rmse",
    },
    MetricTerm {
        canonical: "mse",
        pattern: r"\bmse\b",
    },
    MetricTerm {
        canonical: "mae",
        pattern: r"\bmae\b",
    },
    MetricTerm {
        canonical: "error",
        pattern: r"error(?:\s+rate)?|\berr\.?\b",
    },
    MetricTerm {
        canonical: "loss",
        pattern: r"\bloss\b",
    },
    MetricTerm {
        canonical: "latency",
        pattern: r"latency",
    },
    MetricTerm {
        canonical: "throughput",
        pattern: r"throughput|\bqps\b|tokens?\s*/\s*s",
    },
    MetricTerm {
        canonical: "speedup",
        pattern: r"speed-?up",
    },
    MetricTerm {
        canonical: "energy",
        pattern: r"energy",
    },
    MetricTerm {
        canonical: "memory",
        pattern: r"\bmemory\b",
    },
    MetricTerm {
        canonical: "score",
        pattern: r"\bscore\b",
    },
];

static METRIC_RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    METRICS
        .iter()
        .filter_map(|m| {
            Regex::new(&format!(
                r"(?i)(?:^|[^a-z0-9])(?P<m>{})(?:$|[^a-z0-9])",
                m.pattern
            ))
            .ok()
            .map(|re| (m.canonical, re))
        })
        .collect()
});

/// The first evaluation measure named in `text`: its canonical name and the printed span.
pub fn find_metric(text: &str) -> Option<(String, String)> {
    for (canonical, re) in METRIC_RES.iter() {
        if let Some(caps) = re.captures(text) {
            let printed = caps.name("m")?.as_str().trim().to_string();
            // Group 1 is the whole term; group 2 is the cut-off of `{k}` patterns.
            let name = match caps.get(2).filter(|_| canonical.contains("{k}")) {
                Some(k) => canonical.replace("{k}", &k.as_str().to_ascii_lowercase()),
                None => canonical.to_string(),
            };
            return Some((name, printed));
        }
    }
    None
}

/// Every distinct evaluation measure named in `text` (canonical names).
fn metrics_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text.to_string();
    while let Some((name, printed)) = find_metric(&rest) {
        if !out.contains(&name) {
            out.push(name);
        }
        match rest.find(&printed) {
            Some(at) => rest.replace_range(at..at + printed.len(), " "),
            None => break,
        }
    }
    out
}

/// Size, shot or context qualifiers that are settings, not datasets.
fn is_setting_token(token: &str) -> bool {
    static SETTING: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:\d+(?:\.\d+)?\s*[kmbt]|\d+-?shot|zero-?shot|few-?shot|\d+(?:\.\d+)?[kmbt]?\s*(?:params?|tokens?)|k\s*=\s*\d+|n\s*=\s*\d+|t\s*=\s*\d+|l\s*=\s*\d+|\d+(?:\.\d+)?)$").ok()
    });
    SETTING.as_ref().is_some_and(|re| re.is_match(token.trim()))
}

/// A header naming only an aggregate of other columns (`Avg.`, `Average`, `Overall`).
fn is_aggregate_header(text: &str) -> bool {
    let words: Vec<String> = clean_header(text)
        .split_whitespace()
        .map(|w| {
            w.to_ascii_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect();
    !words.is_empty()
        && words.iter().all(|w| {
            matches!(
                w.as_str(),
                "avg" | "average" | "mean" | "overall" | "all" | "total"
            )
        })
}

/// A data split named in a header (`Valid`, `Test`): it qualifies the column.
fn is_split_name(token: &str) -> bool {
    matches!(
        token
            .to_ascii_lowercase()
            .trim_matches(|c: char| !c.is_alphanumeric()),
        "test" | "dev" | "val" | "valid" | "validation" | "train"
    )
}

/// Words that are not dataset names when left over in a header.
fn is_filler(token: &str) -> bool {
    matches!(
        token
            .to_ascii_lowercase()
            .trim_matches(|c: char| !c.is_alphanumeric()),
        "" | "ours"
            | "avg"
            | "average"
            | "mean"
            | "all"
            | "overall"
            | "total"
            | "test"
            | "dev"
            | "val"
            | "validation"
            | "train"
            | "on"
            | "the"
            | "of"
            | "and"
            | "with"
            | "by"
            | "params"
            | "param"
            | "tokens"
            | "model"
            | "method"
            | "methods"
            | "results"
            | "diff"
            | "delta"
            | "gain"
            | "std"
            | "time"
            | "size"
            | "rank"
            | "type"
    )
}

/// Common benchmark names that do not look like names by their spelling alone.
const KNOWN_DATASETS: &[&str] = &[
    "books",
    "pile",
    "wikitext",
    "wikipedia",
    "imagenet",
    "cifar",
    "mnist",
    "squad",
    "glue",
    "superglue",
    "coco",
    "openwebtext",
    "slimpajama",
    "fineweb",
    "redpajama",
    "lambada",
    "piqa",
    "hellaswag",
    "winogrande",
    "arc",
    "boolq",
    "triviaqa",
    "humaneval",
    "mbpp",
    "gsm8k",
];

/// Whether a header token is spelled like a dataset name: it has a digit (`SIFT1M`,
/// `GSM8K`), two capitals (`PIQA`, `MS-MARCO`), a capital after a lower-case letter
/// (`ImageNet`), is an abbreviation with a period (`Wiki.`, `LMB.`) or a known benchmark.
fn looks_like_dataset(token: &str) -> bool {
    let letters = token.chars().filter(|c| c.is_alphabetic()).count();
    if letters == 0 {
        return false;
    }
    let digit = token.chars().any(|c| c.is_ascii_digit());
    let capitals = token.chars().filter(|c| c.is_uppercase()).count();
    let camel = token
        .chars()
        .zip(token.chars().skip(1))
        .any(|(a, b)| a.is_lowercase() && b.is_uppercase());
    let abbreviation = token.ends_with('.') && token.chars().next().is_some_and(char::is_uppercase);
    let known = KNOWN_DATASETS.contains(
        &token
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase()
            .as_str(),
    );
    digit || capitals >= 2 || camel || abbreviation || known
}

/// A cleaned header text: arrows, units in brackets and markers removed.
fn clean_header(text: &str) -> String {
    static NOISE: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r"[↑↓⇑⇓▲▼†‡*]|\(\s*(?:%|ms|s|mJ|J|GB|MB|x|×)\s*\)|\[\s*(?:%|ms|s|mJ|J|GB|MB)\s*\]",
        )
        .ok()
    });
    let cleaned = match NOISE.as_ref() {
        Some(re) => re.replace_all(text, " ").into_owned(),
        None => text.to_string(),
    };
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Splits one column header into metric, dataset and setting.
pub fn split_header(text: &str) -> ColumnRoles {
    let unit = header_unit(text);
    let cleaned = clean_header(text);
    let mut rest = cleaned.clone();
    let metric = find_metric(&cleaned).map(|(name, printed)| {
        if let Some(at) = rest.find(&printed) {
            rest.replace_range(at..at + printed.len(), " ");
        }
        // Keep the printed form for display; canonical names are derived later.
        let _ = name;
        (printed, Evidence::Header)
    });
    let mut dataset_words: Vec<&str> = Vec::new();
    let mut leftover: Vec<&str> = Vec::new();
    let mut setting: Vec<String> = Vec::new();
    for token in rest.split_whitespace() {
        let token = token
            .trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | ',' | ':' | ';' | '/'));
        if token.is_empty() {
            continue;
        }
        if is_setting_token(token) || is_split_name(token) {
            setting.push(token.to_string());
            continue;
        }
        if token.chars().any(char::is_alphabetic) {
            leftover.push(token);
            if !is_filler(token) {
                dataset_words.push(token);
            }
        }
    }
    // A leftover that is a description ("Total Inference", "Contributed by the IF Layer")
    // is not a dataset: every remaining word must be spelled like a name. Such a
    // description qualifies the metric instead, so it becomes part of the setting.
    let named = !dataset_words.is_empty() && dataset_words.iter().all(|w| looks_like_dataset(w));
    let dataset = named.then(|| (dataset_words.join(" "), Evidence::Header));
    if !named && !dataset_words.is_empty() {
        setting.push(leftover.join(" "));
    }
    ColumnRoles {
        dataset,
        metric,
        setting,
        unit,
    }
}

/// Dataset names a caption gives: `... on SIFT1M`, `... on the ImageNet validation set`.
/// Only a caption naming exactly one is used.
pub fn caption_dataset(caption: &str) -> Option<String> {
    static ON: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"\bon\s+(?:the\s+)?(?P<d>[A-Z][A-Za-z0-9]*(?:[-.][A-Za-z0-9]+)*(?:\s+[A-Z0-9][A-Za-z0-9]*(?:[-.][A-Za-z0-9]+)*)?)")
            .ok()
    });
    let mut found: Vec<String> = Vec::new();
    let on = ON.as_ref()?;
    for caps in on.captures_iter(caption) {
        let Some(d) = caps.name("d") else { continue };
        let name = d.as_str().trim_end_matches('.').to_string();
        // Generic capitalised words are not datasets.
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "table" | "figure" | "section" | "appendix" | "average" | "all" | "our" | "each"
        ) || find_metric(&name).is_some()
            || is_model_name(&name)
        {
            continue;
        }
        if !found.contains(&name) {
            found.push(name);
        }
    }
    match found.as_slice() {
        [one] => Some(one.clone()),
        [] => caption_dataset_token(caption),
        _ => None,
    }
}

/// A model checkpoint name rather than a dataset: a size suffix (`Qwen2.5-3B`,
/// `Llama-2-7B`) or a known model family.
fn is_model_name(name: &str) -> bool {
    static SIZE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)[-_ ]\d+(?:\.\d+)?\s*[bm]$").ok());
    let lower = name.to_ascii_lowercase();
    SIZE.as_ref().is_some_and(|re| re.is_match(name))
        || [
            "gpt", "llama", "qwen", "mistral", "bert", "resnet", "vgg", "vit-", "t5",
        ]
        .iter()
        .any(|family| lower.starts_with(family))
}

/// The one dataset name a caption gives without "on" (`WikiText-103 language model
/// perplexity results ...`): a token mixing letters with a digit and a hyphen, when the
/// caption has exactly one such token. (Plain words such as `WikiText` are too often
/// mentioned in passing to be taken from a caption.)
fn caption_dataset_token(caption: &str) -> Option<String> {
    let mut found: Vec<String> = Vec::new();
    // Skip the "Table N" label.
    let rest = caption
        .split_once([':', '.'])
        .map(|(_, r)| r)
        .unwrap_or(caption);
    for raw in rest.split_whitespace() {
        let token = raw.trim_matches(|c: char| ",.;:()[]{}\"'".contains(c));
        let letters = token.chars().filter(|c| c.is_alphabetic()).count();
        let digit = token.chars().any(|c| c.is_ascii_digit());
        let strong = letters >= 2 && digit && token.contains('-') && !token.contains('/');
        if strong
            && !is_model_name(token)
            && find_metric(token).is_none()
            && !is_setting_token(token)
            && !found.iter().any(|f| f == token)
        {
            found.push(token.to_string());
        }
    }
    match found.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// A dataset named by a section heading: `4.2 Results on ImageNet` (the caption rule),
/// or a short heading made only of dataset-like names (`5.1 LRA`). The deepest heading
/// that names one wins. Generic headings (`4 Experiments`, `4.2 Language Modeling`)
/// name none.
pub fn section_dataset(section_path: &[String]) -> Option<String> {
    static NUMBER: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"^\s*(?:[A-Z]|\d{1,2})(?:\.\d{1,2}){0,3}\.?\s+").ok());
    for heading in section_path.iter().rev() {
        let title = match NUMBER.as_ref() {
            Some(re) => re.replace(heading, "").into_owned(),
            None => heading.clone(),
        };
        if let Some(d) = caption_dataset(&title) {
            if d.split_whitespace().all(looks_like_dataset) {
                return Some(d);
            }
        }
        let words: Vec<&str> = title.split_whitespace().collect();
        let named = !words.is_empty()
            && words.len() <= 3
            && find_metric(&title).is_none()
            && words.iter().all(|w| looks_like_dataset(w) && !is_filler(w));
        if named {
            return Some(words.join(" "));
        }
    }
    None
}

/// A method label with "(ours)" markers removed (`DeltaNet (ours)` → `DeltaNet`), and
/// whether the label is only "Ours" (names no method).
fn method_label(label: &str) -> (String, bool) {
    static OURS: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)\s*[(\[]\s*ours\s*[)\]]\s*").ok());
    let cleaned = match OURS.as_ref() {
        Some(re) => re.replace_all(label, " ").into_owned(),
        None => label.to_string(),
    };
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let bare = cleaned
        .trim_matches(|c: char| !c.is_alphanumeric())
        .eq_ignore_ascii_case("ours");
    if cleaned.is_empty() {
        (label.trim().to_string(), bare)
    } else {
        (cleaned, bare)
    }
}

/// The single metric a caption names, if exactly one.
fn caption_metric(caption: &str) -> Option<String> {
    match metrics_in(caption).as_slice() {
        [_] => find_metric(caption).map(|(_, printed)| printed),
        _ => None,
    }
}

/// Whether a row holds values: a cell with one number, or with several (merged columns).
fn row_has_values(row: &[String]) -> bool {
    row.iter()
        .any(|c| matches!(read_cell(c), Ok(_) | Err(CellIssue::SeveralNumbers)))
}

fn is_word_cell(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && read_cell(t).is_err() && t.chars().any(char::is_alphabetic)
}

/// Whether a row only names settings under a header (`| | 2K | 4K | 8K |`): its first
/// cell is empty and every other filled cell is a size or `k = n` qualifier.
fn is_setting_row(row: &[String], header: &[String]) -> bool {
    static SIZE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)^(?:\d+(?:\.\d+)?\s*[kmbt]|[knlt]\s*=\s*\d+)$").ok());
    let Some(size) = SIZE.as_ref() else {
        return false;
    };
    let filled: Vec<(usize, &str)> = row
        .iter()
        .map(|c| c.trim())
        .enumerate()
        .filter(|(_, c)| !c.is_empty())
        .collect();
    let sizes = filled.iter().filter(|(_, c)| size.is_match(c)).count();
    let repeated = |i: usize, c: &str| header.get(i).is_some_and(|h| h.trim() == c);
    row.first().is_some_and(|c| c.trim().is_empty())
        && sizes >= 2
        && filled
            .iter()
            .all(|(i, c)| size.is_match(c) || repeated(*i, c))
}

/// The words of a column header other than its measure and settings, trimmed of
/// punctuation (what is left to describe the column).
fn header_words(text: &str) -> Vec<String> {
    let cleaned = clean_header(text);
    let mut rest = cleaned.clone();
    if let Some((_, printed)) = find_metric(&cleaned) {
        if let Some(at) = rest.find(&printed) {
            rest.replace_range(at..at + printed.len(), " ");
        }
    }
    rest.split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | ',' | ':' | ';' | '/'))
                .to_string()
        })
        .filter(|t| t.chars().any(char::is_alphabetic) && !is_setting_token(t))
        .collect()
}

/// Words that name a split or a model size rather than describe a column (`Valid`,
/// `Test`, `small`), and fillers.
fn is_split_word(word: &str) -> bool {
    is_filler(word)
        || matches!(
            word.to_ascii_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric()),
            "valid" | "small" | "medium" | "large" | "base" | "tiny" | "xl"
        )
}

/// Whether a label column's header says its cells are datasets (`Dataset`, `Benchmark`).
fn is_dataset_header(text: &str) -> bool {
    let lowest = text.rsplit(" / ").next().unwrap_or(text);
    matches!(
        lowest
            .trim()
            .trim_end_matches(['.', ':'])
            .to_ascii_lowercase()
            .as_str(),
        "dataset" | "datasets" | "data" | "benchmark" | "benchmarks" | "task" | "tasks"
    )
}

/// Marker characters printed after a label (`Samba∗`, `Ours†`).
fn strip_markers(label: &str) -> String {
    label
        .trim_end_matches(|c: char| "*∗†‡§¶".contains(c) || c.is_whitespace())
        .to_string()
}

/// A trailing group label run into a row label by the text layer
/// (`DeltaNet [−1, 1] 370M params`): the method, and the group that starts after it.
fn split_trailing_group(label: &str) -> (String, Option<String>) {
    static GROUP: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?P<m>.*\S)\s+(?P<g>\d+(?:\.\d+)?\s*[kmbt]\s+params?(?:\s*/\s*\d+(?:\.\d+)?\s*[kmbt]\s+tokens)?)$").ok()
    });
    match GROUP.as_ref().and_then(|re| re.captures(label)) {
        Some(c) => (c["m"].to_string(), Some(c["g"].to_string())),
        None => (label.to_string(), None),
    }
}

/// A row label that continues the method above it: a variant in parentheses
/// (`(w. conv)`) or an addition (`+ Sliding Attn`).
fn is_continuation(label: &str) -> bool {
    label.starts_with('(') || label.starts_with('+')
}

/// The method a continuation label extends: the label above without its parenthetical
/// variant (`GLA (w/o. conv)` → `GLA`).
fn base_method(label: &str) -> String {
    match label.find(" (") {
        Some(at) => label[..at].trim().to_string(),
        None => label.trim().to_string(),
    }
}

/// Reads a table. `model` supplies roles for columns the rules could not name.
pub fn read_table(table: &TableInput, model: Option<&HeaderRoles>) -> Reading {
    let width = std::iter::once(table.header.len())
        .chain(table.rows.iter().map(Vec::len))
        .max()
        .unwrap_or(0);
    if width < 2 || table.rows.is_empty() {
        return skipped("the table has fewer than two columns or no body rows");
    }
    let mut header_rows: Vec<Vec<String>> = vec![table.header.clone()];
    let mut first_body = 0;
    for row in &table.rows {
        if (!row_has_values(row) || is_setting_row(row, &table.header))
            && first_body < 3
            && row.iter().filter(|c| !c.trim().is_empty()).count() > 1
        {
            header_rows.push(row.clone());
            first_body += 1;
        } else {
            break;
        }
    }
    let body: Vec<&Vec<String>> = table.rows.iter().skip(first_body).collect();
    if body.is_empty() {
        return skipped("no body rows hold numbers");
    }
    if header_rows[0]
        .iter()
        .filter(|c| read_cell(c).is_ok())
        .count()
        * 2
        > header_rows[0]
            .iter()
            .filter(|c| !c.trim().is_empty())
            .count()
            .max(1)
    {
        return skipped("the column headers are numbers: the table's header was not recovered");
    }
    // Label columns: the leading columns whose body cells are mostly words.
    let mut label_columns = 0;
    for column in 0..width {
        let cells: Vec<&str> = body
            .iter()
            .filter_map(|r| r.get(column).map(String::as_str))
            .filter(|c| !c.trim().is_empty())
            .collect();
        let words = cells.iter().filter(|c| is_word_cell(c)).count();
        if !cells.is_empty() && words * 2 > cells.len() {
            label_columns = column + 1;
        } else {
            break;
        }
    }
    if label_columns == 0 {
        return skipped("no column holds method names");
    }
    if label_columns == width {
        return skipped("no column holds numbers");
    }
    // Side-by-side tables (`Model | Dataset | T | Acc. | Model | Dataset | T | Acc.`): a
    // word column after the value columns starts a second table, read on its own so its
    // values are never paired with the first table's row labels.
    let word_column = |column: usize| {
        let cells: Vec<&str> = body
            .iter()
            .filter_map(|r| r.get(column).map(String::as_str))
            .filter(|c| !c.trim().is_empty())
            .collect();
        !cells.is_empty() && cells.iter().filter(|c| is_word_cell(c)).count() * 2 > cells.len()
    };
    if let Some(split) = (label_columns + 1..width).find(|&c| word_column(c) && !word_column(c - 1))
    {
        let left = read_table(&slice_columns(table, 0, split), model);
        let right = read_table(&slice_columns(table, split, width), None);
        return merge_readings(left, right, split);
    }
    let header_text = |column: usize| -> String {
        let mut parts: Vec<&str> = Vec::new();
        for row in &header_rows {
            if let Some(cell) = row.get(column).map(|c| c.trim()).filter(|c| !c.is_empty()) {
                if !parts.contains(&cell) {
                    parts.push(cell);
                }
            }
        }
        parts.join(" ")
    };

    // A label column headed `Dataset` names each row's dataset; the other label columns
    // name the method.
    let dataset_column = (0..label_columns)
        .find(|&c| is_dataset_header(&header_text(c)))
        .filter(|_| label_columns >= 2);

    // Transposed: datasets with their metric in the row labels, methods in the columns.
    let label_metrics = body
        .iter()
        .filter(|r| {
            let label = r[..label_columns.min(r.len())].join(" ");
            find_metric(&label).is_some()
        })
        .count();
    let header_metrics = (label_columns..width)
        .filter(|&c| find_metric(&header_text(c)).is_some())
        .count();
    if label_metrics * 2 > body.len() && header_metrics == 0 {
        return read_transposed(table, &header_rows, &body, first_body, label_columns, width);
    }

    let caption = table.caption();
    let caption_evidence = |parser: bool| {
        if parser {
            Evidence::Caption
        } else {
            Evidence::NearbyCaption
        }
    };
    // Columns that are tasks rather than measures (`Compress | Fuzzy Recall | Memorize`):
    // a measure word inside some task names is not the column's metric.
    let headed: Vec<usize> = (label_columns..width)
        .filter(|&c| !header_text(c).is_empty())
        .collect();
    let with_metric = headed
        .iter()
        .filter(|&&c| find_metric(&header_text(c)).is_some())
        .count();
    let task_columns = with_metric > 0 && with_metric * 2 < headed.len();
    let mut roles: BTreeMap<usize, ColumnRoles> = BTreeMap::new();
    let mut unresolved = Vec::new();
    for column in label_columns..width {
        let text = header_text(column);
        if is_aggregate_header(&text) {
            // An average over the table's other columns is derived, not a reported result.
            continue;
        }
        let mut r = split_header(&text);
        if task_columns {
            r = ColumnRoles {
                dataset: None,
                metric: None,
                setting: vec![text.clone()],
                unit: r.unit,
            };
        }
        if dataset_column.is_some() {
            // Each row names its dataset; a name in the header qualifies the column.
            if let Some((d, _)) = r.dataset.take() {
                r.setting.insert(0, d);
            }
        }
        // A header that describes its column in words of its own (`Prms. in M.`, `χ`)
        // does not take the caption's measure, and one whose words are all spelled like
        // a name (`Hella.`) does not take the caption's or section's dataset.
        let words = header_words(&text);
        let described = words
            .iter()
            .any(|w| !is_split_word(w) && !looks_like_dataset(w));
        let naming: Vec<&String> = words.iter().filter(|w| !is_split_word(w)).collect();
        let names_dataset = !naming.is_empty() && naming.iter().all(|w| looks_like_dataset(w));
        if r.metric.is_none() && !described {
            r.metric = caption
                .and_then(|(c, parser)| caption_metric(c).map(|m| (m, caption_evidence(parser))));
        }
        if r.dataset.is_none() && dataset_column.is_none() && !names_dataset {
            r.dataset = caption
                .and_then(|(c, parser)| caption_dataset(c).map(|d| (d, caption_evidence(parser))));
        }
        if r.dataset.is_none() && dataset_column.is_none() && !names_dataset {
            r.dataset = section_dataset(&table.section_path).map(|d| (d, Evidence::Section));
        }
        if let Some(model) = model {
            if let Some(named) = model.columns.iter().find(|c| c.index == column) {
                if r.metric.is_none() {
                    r.metric = named.metric.clone().map(|m| (m, Evidence::Model));
                }
                if r.dataset.is_none() {
                    r.dataset = named.dataset.clone().map(|d| (d, Evidence::Model));
                }
                if let Some(setting) = named.setting.clone().filter(|s| !s.trim().is_empty()) {
                    if !r.setting.contains(&setting) {
                        r.setting.push(setting);
                    }
                }
            }
        }
        if r.metric.is_none() || (r.dataset.is_none() && dataset_column.is_none()) {
            unresolved.push(column);
        }
        roles.insert(column, r);
    }

    let table_label = table.label();
    let mut reading = Reading {
        unresolved,
        ..Reading::default()
    };
    let mut group: Option<String> = None;
    let mut base: Option<String> = None;
    // A label cell spanning rows is often printed once: a row whose first label cell is
    // empty while a later one is filled (`| | delta |` under `| Performer | sum |`)
    // continues the label above.
    let mut above: Vec<String> = vec![String::new(); label_columns];
    for (index, row) in body.iter().enumerate() {
        let mut row: Vec<String> = (*row).clone();
        let partial = label_columns >= 2
            && row.first().is_some_and(|c| c.trim().is_empty())
            && row[1..label_columns.min(row.len())]
                .iter()
                .any(|c| !c.trim().is_empty());
        if partial {
            for (column, cell) in row.iter_mut().enumerate().take(label_columns) {
                if !cell.trim().is_empty() {
                    break;
                }
                cell.clone_from(&above[column]);
            }
        }
        for (column, cell) in row.iter().enumerate().take(label_columns) {
            if !cell.trim().is_empty() {
                above[column].clone_from(cell);
            }
        }
        let row = &row;
        let row_dataset = dataset_column
            .and_then(|c| row.get(c))
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());
        let label = row[..label_columns.min(row.len())]
            .iter()
            .enumerate()
            .filter(|(c, _)| Some(*c) != dataset_column)
            .map(|(_, c)| c.trim())
            .filter(|c| !c.is_empty() && read_cell(c) != Err(CellIssue::Empty))
            .collect::<Vec<_>>()
            .join(" ");
        let (label, next_group) = split_trailing_group(&strip_markers(&label));
        let values: Vec<(usize, &str)> = (label_columns..width)
            .filter_map(|c| row.get(c).map(|t| (c, t.as_str())))
            .filter(|(_, t)| !t.trim().is_empty())
            .collect();
        if values.is_empty() {
            if !label.is_empty() {
                group = Some(label);
                base = None;
            }
            continue;
        }
        if label.is_empty() {
            continue;
        }
        let label = if is_continuation(&label) {
            match &base {
                Some(b) => format!("{b} {label}"),
                None => label,
            }
        } else {
            base = Some(base_method(&label));
            label
        };
        let row_group = group.clone();
        if next_group.is_some() {
            group = next_group;
            base = None;
        }
        for (column, text) in values {
            let Some(r) = roles.get(&column) else {
                continue;
            };
            let dataset = match &row_dataset {
                Some(d) => Some((d.clone(), Evidence::Header)),
                None if dataset_column.is_some() => None,
                None => r.dataset.clone(),
            };
            let (Some((metric, metric_evidence)), Some((dataset, dataset_evidence))) =
                (r.metric.clone(), dataset)
            else {
                continue;
            };
            let number = match read_cell(text) {
                Ok(n) => n,
                Err(CellIssue::SeveralNumbers) => {
                    reading.ambiguous_cells += 1;
                    continue;
                }
                Err(_) => continue,
            };
            let mut setting: Vec<String> = Vec::new();
            if let Some(label) = &table_label {
                setting.push(label.clone());
            }
            if let Some(group) = &row_group {
                setting.push(group.clone());
            }
            setting.extend(r.setting.iter().cloned());
            let mut confidence = BASE_CONFIDENCE;
            for evidence in [metric_evidence, dataset_evidence] {
                confidence = match evidence {
                    Evidence::Header => confidence,
                    Evidence::Caption => confidence - CAPTION_PENALTY,
                    Evidence::NearbyCaption | Evidence::Section => confidence - NEARBY_PENALTY,
                    Evidence::Model => confidence.min(MODEL_CONFIDENCE),
                };
            }
            let (method, only_ours) = method_label(&label);
            if only_ours {
                confidence = confidence.min(OURS_CONFIDENCE);
            }
            let unit = number.unit.clone().or_else(|| r.unit.clone());
            reading.candidates.push(Candidate {
                method,
                dataset,
                dataset_evidence,
                metric,
                metric_evidence,
                cell_text: text.trim().to_string(),
                unit,
                setting: setting.join("; "),
                page: table.page,
                row: index,
                column,
                cell_box: table.cell_box(first_body + index + 1, column),
                confidence: (confidence * 100.0).round() / 100.0,
                number,
            });
        }
    }
    if reading.candidates.is_empty() {
        reading.skipped = Some(if !reading.unresolved.is_empty() {
            "no dataset or metric is named in the column headers or the caption".to_string()
        } else if reading.ambiguous_cells > 0 {
            "every value cell holds several numbers (merged columns)".to_string()
        } else {
            "no cell under a dataset and metric holds a single number".to_string()
        });
    }
    reading
}

/// Columns `from..to` of a table as a table of their own.
fn slice_columns(table: &TableInput, from: usize, to: usize) -> TableInput {
    let cut = |row: &Vec<String>| -> Vec<String> {
        (from..to)
            .map(|c| row.get(c).cloned().unwrap_or_default())
            .collect()
    };
    TableInput {
        header: cut(&table.header),
        rows: table.rows.iter().map(cut).collect(),
        cell_boxes: table
            .cell_boxes
            .iter()
            .map(|row| (from..to).map(|c| row.get(c).copied().flatten()).collect())
            .collect(),
        ..table.clone()
    }
}

/// The readings of two side-by-side tables as one; the right one's columns are offset
/// by `offset`.
fn merge_readings(left: Reading, right: Reading, offset: usize) -> Reading {
    let mut out = left;
    out.candidates
        .extend(right.candidates.into_iter().map(|mut c| {
            c.column += offset;
            c
        }));
    out.unresolved
        .extend(right.unresolved.into_iter().map(|c| c + offset));
    out.ambiguous_cells += right.ambiguous_cells;
    out.skipped = if out.candidates.is_empty() {
        out.skipped.or(right.skipped)
    } else {
        None
    };
    out
}

fn read_transposed(
    table: &TableInput,
    header_rows: &[Vec<String>],
    body: &[&Vec<String>],
    first_body: usize,
    label_columns: usize,
    width: usize,
) -> Reading {
    let mut reading = Reading::default();
    let table_label = table.label();
    for column in label_columns..width {
        let method = header_rows
            .iter()
            .filter_map(|r| r.get(column).map(|c| c.trim()))
            .filter(|c| !c.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if method.is_empty() || read_cell(&method).is_ok() {
            continue;
        }
        for (index, row) in body.iter().enumerate() {
            let label = row[..label_columns.min(row.len())]
                .iter()
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let Some((_, printed)) = find_metric(&label) else {
                continue;
            };
            let roles = split_header(&label);
            let Some((dataset, _)) = roles.dataset else {
                continue;
            };
            let Some(text) = row.get(column) else {
                continue;
            };
            let number = match read_cell(text) {
                Ok(n) => n,
                Err(CellIssue::SeveralNumbers) => {
                    reading.ambiguous_cells += 1;
                    continue;
                }
                Err(_) => continue,
            };
            let mut setting: Vec<String> = table_label.iter().cloned().collect();
            setting.extend(roles.setting.iter().cloned());
            reading.candidates.push(Candidate {
                method: method.clone(),
                dataset,
                dataset_evidence: Evidence::Header,
                metric: printed,
                metric_evidence: Evidence::Header,
                cell_text: text.trim().to_string(),
                unit: number.unit.clone().or_else(|| roles.unit.clone()),
                setting: setting.join("; "),
                page: table.page,
                row: index,
                column,
                cell_box: table.cell_box(first_body + index + 1, column),
                confidence: BASE_CONFIDENCE,
                number,
            });
        }
    }
    if reading.candidates.is_empty() {
        reading.skipped = Some("no transposed value could be read".to_string());
    }
    reading
}

fn skipped(reason: &str) -> Reading {
    Reading {
        skipped: Some(reason.to_string()),
        ..Reading::default()
    }
}

/// The text a model's labels must come from: every header and label cell, the caption
/// and the section path.
pub fn table_text(table: &TableInput) -> String {
    let mut parts: Vec<&str> = table.header.iter().map(String::as_str).collect();
    for row in &table.rows {
        parts.extend(row.iter().map(String::as_str));
    }
    if let Some(c) = &table.caption {
        parts.push(c);
    }
    if let Some(c) = &table.nearby_caption {
        parts.push(c);
    }
    parts.extend(table.section_path.iter().map(String::as_str));
    parts.join("\n")
}

/// Keeps only the model's labels that occur verbatim (case-insensitively, whitespace
/// collapsed) in the table text and that are not numbers; drops columns that do not
/// exist. Returns the verified roles and how many labels were refused.
pub fn verify_roles(table: &TableInput, roles: HeaderRoles, width: usize) -> (HeaderRoles, usize) {
    let haystack = table_text(table)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut refused = 0;
    let mut out = HeaderRoles::default();
    for column in roles.columns {
        if column.index >= width {
            refused += 1;
            continue;
        }
        let mut keep = |label: Option<String>| -> Option<String> {
            let clean = label?.split_whitespace().collect::<Vec<_>>().join(" ");
            if clean.is_empty() {
                return None;
            }
            if read_cell(&clean).is_ok() || !haystack.contains(&clean.to_lowercase()) {
                refused += 1;
                return None;
            }
            Some(clean)
        };
        let metric = keep(column.metric);
        let dataset = keep(column.dataset);
        let setting = keep(column.setting);
        if metric.is_some() || dataset.is_some() || setting.is_some() {
            out.columns.push(ModelColumn {
                index: column.index,
                metric,
                dataset,
                setting,
            });
        }
    }
    (out, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(header: &[&str], rows: &[&[&str]], caption: Option<&str>) -> TableInput {
        TableInput {
            page: 6,
            bbox: None,
            caption: caption.map(str::to_string),
            nearby_caption: None,
            section_path: vec![],
            header: header.iter().map(|s| s.to_string()).collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect(),
            cell_boxes: vec![],
        }
    }

    #[test]
    fn metrics_are_found_with_their_cut_off() {
        assert_eq!(
            find_metric("R@10").map(|m| m.0).as_deref(),
            Some("recall@10")
        );
        assert_eq!(
            find_metric("Recall @ 100").map(|m| m.0).as_deref(),
            Some("recall@100")
        );
        assert_eq!(
            find_metric("nDCG@10").map(|m| m.0).as_deref(),
            Some("ndcg@10")
        );
        assert_eq!(
            find_metric("Wiki. ppl ↓").map(|m| m.0).as_deref(),
            Some("perplexity")
        );
        assert_eq!(
            find_metric("acc n ↑").map(|m| m.0).as_deref(),
            Some("accuracy")
        );
        assert_eq!(
            find_metric("Top-1 Acc.").map(|m| m.0).as_deref(),
            Some("top-1-accuracy")
        );
        assert_eq!(
            find_metric("MMLU-Pro (EM)").map(|m| m.0).as_deref(),
            Some("exact-match")
        );
        assert_eq!(
            find_metric("HumanEval (Pass@1)").map(|m| m.0).as_deref(),
            Some("pass@1")
        );
        assert_eq!(
            find_metric("Total Inference Energy")
                .map(|m| m.0)
                .as_deref(),
            Some("energy")
        );
        assert_eq!(find_metric("Mamba"), None);
        assert_eq!(find_metric("SIFT1M"), None);
        assert_eq!(find_metric("Embed. dim."), None);
    }

    #[test]
    fn headers_split_into_metric_dataset_and_setting() {
        let r = split_header("SIFT1M R@10 (%)");
        assert_eq!(r.metric, Some(("R@10".into(), Evidence::Header)));
        assert_eq!(r.dataset, Some(("SIFT1M".into(), Evidence::Header)));
        assert_eq!(r.unit.as_deref(), Some("%"));
        let r = split_header("Wiki. ppl ↓");
        assert_eq!(r.metric.map(|m| m.0).as_deref(), Some("ppl"));
        assert_eq!(r.dataset.map(|d| d.0).as_deref(), Some("Wiki."));
        let r = split_header("S-NIAH-1 2K acc");
        assert_eq!(r.setting, vec!["2K".to_string()]);
        assert_eq!(r.dataset.map(|d| d.0).as_deref(), Some("S-NIAH-1"));
        let r = split_header("Ppl.");
        assert_eq!(r.dataset, None);
        let r = split_header("Total Inference Energy");
        assert_eq!(
            (r.metric.map(|m| m.0), r.dataset),
            (Some("Energy".into()), None)
        );
        assert_eq!(r.setting, vec!["Total Inference".to_string()]);
        assert_eq!(
            split_header("Energy Contributed by the IF Layer").dataset,
            None
        );
        assert_eq!(
            split_header("PIQA acc").dataset.map(|d| d.0).as_deref(),
            Some("PIQA")
        );
        assert_eq!(
            split_header("Books ppl").dataset.map(|d| d.0).as_deref(),
            Some("Books")
        );
        assert_eq!(
            caption_dataset("Table 2: Recall of graph indexes on SIFT1M."),
            Some("SIFT1M".into())
        );
        assert_eq!(
            caption_dataset("Table 2: Results on SIFT1M and on GIST1M."),
            None
        );
        assert_eq!(caption_dataset("Table 5: Architectural Details."), None);
    }

    #[test]
    fn a_clean_results_table_reads_every_value_exactly() {
        let t = table(
            &["Method", "SIFT1M R@10", "SIFT1M QPS", "GIST1M R@10"],
            &[
                &["HNSW", "95.3", "12,400", "88.10*"],
                &["IVF-PQ", "−", "30,100", "71.4 ± 0.3"],
                &["Ours", "97.1", "28 750", "90.2"],
            ],
            Some("Table 2: Graph and quantisation indexes."),
        );
        let reading = read_table(&t, None);
        let got: Vec<(String, String, String, String)> = reading
            .candidates
            .iter()
            .map(|c| {
                (
                    c.method.clone(),
                    c.dataset.clone(),
                    c.metric.clone(),
                    c.number.decimal.clone(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("HNSW".into(), "SIFT1M".into(), "R@10".into(), "95.3".into()),
                ("HNSW".into(), "SIFT1M".into(), "QPS".into(), "12400".into()),
                (
                    "HNSW".into(),
                    "GIST1M".into(),
                    "R@10".into(),
                    "88.10".into()
                ),
                (
                    "IVF-PQ".into(),
                    "SIFT1M".into(),
                    "QPS".into(),
                    "30100".into()
                ),
                (
                    "IVF-PQ".into(),
                    "GIST1M".into(),
                    "R@10".into(),
                    "71.4".into()
                ),
                ("Ours".into(), "SIFT1M".into(), "R@10".into(), "97.1".into()),
                ("Ours".into(), "GIST1M".into(), "R@10".into(), "90.2".into()),
            ]
        );
        // "28 750" holds two numbers: refused, never guessed.
        assert_eq!(reading.ambiguous_cells, 1);
        for c in &reading.candidates {
            assert!(c.cell_text.contains(&c.number.lexeme));
            // "Ours" names no method: its values wait for review.
            let expected = if c.method == "Ours" { 0.7 } else { 0.9 };
            assert_eq!(c.confidence, expected);
            assert_eq!(c.setting, "Table 2");
        }
    }

    #[test]
    fn multi_row_headers_groups_and_caption_datasets() {
        let t = table(
            &["", "Recall@10", "Recall@100"],
            &[
                &["Model", "", ""],
                &["Small models", "", ""],
                &["A", "0.81", "0.95"],
                &["Large models", "", ""],
                &["B", "0.85", "0.97"],
            ],
            Some("Table 4: Retrieval on MS-MARCO."),
        );
        let reading = read_table(&t, None);
        assert_eq!(reading.candidates.len(), 4);
        let b = &reading.candidates[2];
        assert_eq!((b.method.as_str(), b.dataset.as_str()), ("B", "MS-MARCO"));
        assert_eq!(b.dataset_evidence, Evidence::Caption);
        assert_eq!(b.setting, "Table 4; Large models");
        assert_eq!(b.confidence, 0.75);
    }

    #[test]
    fn tables_without_named_datasets_or_headers_are_skipped_with_reasons() {
        let no_dataset = table(
            &["Configuration", "Ppl.", "Diff."],
            &[&["Linear attention", "15.91", ""]],
            None,
        );
        let reading = read_table(&no_dataset, None);
        assert!(reading.candidates.is_empty());
        assert_eq!(reading.unresolved, vec![1, 2]);
        assert!(reading.skipped.unwrap().contains("no dataset"));

        let numeric_header = table(
            &["Mamba2", "98.6", "61.4"],
            &[&["DeltaNet", "96.8", "98.8"]],
            None,
        );
        assert!(read_table(&numeric_header, None)
            .skipped
            .unwrap()
            .contains("header"));

        let merged = table(
            &["Model", "Wiki. ppl", "LMB. acc"],
            &[&["GLA", "28.39 42.69", "31.0 63.3"]],
            None,
        );
        let reading = read_table(&merged, None);
        assert!(reading.candidates.is_empty());
        assert_eq!(reading.ambiguous_cells, 2);
    }

    #[test]
    fn section_headings_name_datasets_only_when_they_name_one() {
        let path = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            section_dataset(&path(&["4 Experiments", "4.2 Results on ImageNet"])),
            Some("ImageNet".into())
        );
        assert_eq!(
            section_dataset(&path(&["5 Experiments", "5.1 LRA"])),
            Some("LRA".into())
        );
        assert_eq!(
            section_dataset(&path(&["4 Experiments", "4.2 Language Modeling"])),
            None
        );
        assert_eq!(section_dataset(&path(&["3 Recall@10 Analysis"])), None);
        assert_eq!(section_dataset(&[]), None);

        let mut t = table(
            &["Model", "Ppl."],
            &[&["DeltaNet", "17.7"], &["GLA", "18.2"]],
            Some("Table 3: Perplexity."),
        );
        t.section_path = path(&["4 Experiments", "4.1 Results on WikiText-103"]);
        let reading = read_table(&t, None);
        assert_eq!(reading.candidates.len(), 2);
        assert!(reading
            .candidates
            .iter()
            .all(|c| c.dataset == "WikiText-103"
                && c.dataset_evidence == Evidence::Section
                && c.confidence == 0.65));
    }

    #[test]
    fn ours_markers_are_plain_text_and_bare_ours_goes_to_review() {
        assert_eq!(method_label("DeltaNet (ours)"), ("DeltaNet".into(), false));
        assert_eq!(method_label("DeltaNet [Ours]"), ("DeltaNet".into(), false));
        assert_eq!(method_label("Ours"), ("Ours".into(), true));
        assert_eq!(method_label("Ours*"), ("Ours*".into(), true));
        assert_eq!(method_label("Hours"), ("Hours".into(), false));
        let t = table(
            &["Method", "SIFT1M R@10"],
            &[
                &["HNSW", "95.3"],
                &["Ours", "97.1"],
                &["IVF (ours)", "96.0"],
            ],
            None,
        );
        let got: Vec<(String, f64)> = read_table(&t, None)
            .candidates
            .iter()
            .map(|c| (c.method.clone(), c.confidence))
            .collect();
        assert_eq!(
            got,
            vec![
                ("HNSW".into(), 0.9),
                ("Ours".into(), 0.7),
                ("IVF".into(), 0.9)
            ]
        );
    }

    #[test]
    fn flattened_multi_row_headers_split_into_dataset_and_metric() {
        let t = table(
            &["Method", "SIFT1M / R@10", "SIFT1M / QPS", "GIST1M / R@10"],
            &[&["HNSW", "95.3", "12,400", "88.1 ± 0.4"]],
            None,
        );
        let got: Vec<(String, String, String, Option<String>)> = read_table(&t, None)
            .candidates
            .iter()
            .map(|c| {
                (
                    c.dataset.clone(),
                    c.metric.clone(),
                    c.number.decimal.clone(),
                    c.number.spread.clone(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("SIFT1M".into(), "R@10".into(), "95.3".into(), None),
                ("SIFT1M".into(), "QPS".into(), "12400".into(), None),
                (
                    "GIST1M".into(),
                    "R@10".into(),
                    "88.1".into(),
                    Some("0.4".into())
                ),
            ]
        );
    }

    #[test]
    fn labels_printed_once_carry_down_and_splits_qualify_columns() {
        // 2102.11174 Table 2: "Performer" is printed once for its two update rules.
        let t = table(
            &["", "Update Rule", "small / Valid", "small / Test"],
            &[
                &["Transformer", "-", "33.0", "34.1"],
                &["Performer", "sum", "39.0", "39.6"],
                &["", "delta", "36.1", "37.2"],
            ],
            Some("Table 2. WikiText-103 language model perplexity results."),
        );
        let got: Vec<(String, String, String, String)> = read_table(&t, None)
            .candidates
            .iter()
            .map(|c| {
                (
                    c.method.clone(),
                    c.dataset.clone(),
                    c.number.lexeme.clone(),
                    c.setting.clone(),
                )
            })
            .collect();
        let row = |m: &str, v: &str, s: &str| {
            (
                m.to_string(),
                "WikiText-103".to_string(),
                v.to_string(),
                format!("Table 2; {s}; small"),
            )
        };
        assert_eq!(
            got,
            vec![
                row("Transformer", "33.0", "Valid"),
                row("Transformer", "34.1", "Test"),
                row("Performer sum", "39.0", "Valid"),
                row("Performer sum", "39.6", "Test"),
                row("Performer delta", "36.1", "Valid"),
                row("Performer delta", "37.2", "Test"),
            ]
        );
    }

    #[test]
    fn side_by_side_tables_and_dataset_columns_are_read_apart() {
        let t = table(
            &["Model", "Dataset", "Acc.", "Model", "Dataset", "Acc."],
            &[
                &[
                    "VGG-16",
                    "CIFAR-10",
                    "93.95%",
                    "ResNet-18",
                    "CIFAR-100",
                    "65.48%",
                ],
                &[
                    "ResNet-34",
                    "ImageNet",
                    "74.31%",
                    "VGG-16",
                    "ImageNet",
                    "73.98 %",
                ],
            ],
            None,
        );
        let got: Vec<(String, String, String, usize)> = read_table(&t, None)
            .candidates
            .iter()
            .map(|c| {
                (
                    c.method.clone(),
                    c.dataset.clone(),
                    c.number.decimal.clone(),
                    c.column,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("VGG-16".into(), "CIFAR-10".into(), "93.95".into(), 2),
                ("ResNet-34".into(), "ImageNet".into(), "74.31".into(), 2),
                ("ResNet-18".into(), "CIFAR-100".into(), "65.48".into(), 5),
                ("VGG-16".into(), "ImageNet".into(), "73.98".into(), 5),
            ]
        );
    }

    #[test]
    fn task_columns_and_size_rows_are_not_misread() {
        // MAD-style table: tasks, a measure word inside some task names.
        let tasks = table(
            &[
                "Model",
                "Compress",
                "Fuzzy Recall",
                "In-Context Recall",
                "Memorize",
                "Average",
            ],
            &[&["Mamba", "52.7", "6.7", "90.4", "89.5", "69.3"]],
            None,
        );
        assert!(read_table(&tasks, None).candidates.is_empty());
        // A row of context sizes under spanning dataset headers continues the header.
        let niah = table(
            &["Model", "S-NIAH-PK", "S-NIAH-PK", "Average"],
            &[
                &["", "2K", "4K", "Average"],
                &["Mamba2", "98.6", "61.4", "52.0"],
            ],
            Some("Table 3: Accuracy on NIAH tasks."),
        );
        let got: Vec<(String, String)> = read_table(&niah, None)
            .candidates
            .iter()
            .map(|c| (c.number.lexeme.clone(), c.setting.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("98.6".into(), "Table 3; 2K".into()),
                ("61.4".into(), "Table 3; 4K".into())
            ]
        );
    }

    #[test]
    fn continuation_labels_and_run_in_groups_name_their_method() {
        let t = table(
            &["Model", "Wiki. ppl"],
            &[
                &["340M params", ""],
                &["GLA (w/o. conv)", "28.65"],
                &["(w. conv)", "29.47"],
                &["DeltaNet [−1, 1] 370M params", "28.24"],
                &["Mamba [0, 1]", "24.84"],
                &["Samba∗", "20.63"],
            ],
            None,
        );
        let got: Vec<(String, String)> = read_table(&t, None)
            .candidates
            .iter()
            .map(|c| (c.method.clone(), c.setting.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("GLA (w/o. conv)".into(), "340M params".into()),
                ("GLA (w. conv)".into(), "340M params".into()),
                ("DeltaNet [−1, 1]".into(), "340M params".into()),
                ("Mamba [0, 1]".into(), "370M params".into()),
                ("Samba".into(), "370M params".into()),
            ]
        );
    }

    #[test]
    fn transposed_tables_read_methods_from_columns() {
        let t = table(
            &["Benchmark", "Model A", "Model B"],
            &[
                &["MMLU-Pro (EM)", "68.3", "73.5"],
                &["HumanEval (Pass@1)", "69.5", "-"],
            ],
            None,
        );
        let reading = read_table(&t, None);
        let got: Vec<(String, String, String, String)> = reading
            .candidates
            .iter()
            .map(|c| {
                (
                    c.method.clone(),
                    c.dataset.clone(),
                    c.metric.clone(),
                    c.number.lexeme.clone(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "Model A".into(),
                    "MMLU-Pro".into(),
                    "EM".into(),
                    "68.3".into()
                ),
                (
                    "Model A".into(),
                    "HumanEval".into(),
                    "Pass@1".into(),
                    "69.5".into()
                ),
                (
                    "Model B".into(),
                    "MMLU-Pro".into(),
                    "EM".into(),
                    "73.5".into()
                ),
            ]
        );
    }

    #[test]
    fn model_roles_are_verified_against_the_table_text() {
        let t = table(
            &["Configuration", "Ppl.", "Diff."],
            &[
                &["Linear attention", "15.91", ""],
                &["TTT", "15.23", "−0.68"],
            ],
            Some("Table 1: Ablations; every model is trained and evaluated with PG19."),
        );
        let roles = HeaderRoles {
            columns: vec![
                ModelColumn {
                    index: 1,
                    metric: Some("Ppl.".into()),
                    dataset: Some("PG19".into()),
                    setting: None,
                },
                // Invented dataset and an out-of-range column are refused.
                ModelColumn {
                    index: 2,
                    metric: Some("Diff.".into()),
                    dataset: Some("Pile".into()),
                    setting: None,
                },
                ModelColumn {
                    index: 9,
                    metric: Some("Ppl.".into()),
                    dataset: Some("PG19".into()),
                    setting: None,
                },
            ],
        };
        let (verified, refused) = verify_roles(&t, roles, 3);
        assert_eq!(refused, 2);
        let reading = read_table(&t, Some(&verified));
        let got: Vec<(String, String, String, f64)> = reading
            .candidates
            .iter()
            .map(|c| {
                (
                    c.method.clone(),
                    c.dataset.clone(),
                    c.number.lexeme.clone(),
                    c.confidence,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "Linear attention".into(),
                    "PG19".into(),
                    "15.91".into(),
                    MODEL_CONFIDENCE
                ),
                (
                    "TTT".into(),
                    "PG19".into(),
                    "15.23".into(),
                    MODEL_CONFIDENCE
                ),
            ]
        );
    }
}
