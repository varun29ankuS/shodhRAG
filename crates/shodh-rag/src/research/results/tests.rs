use std::collections::BTreeMap;
use std::sync::Arc;

use super::interpret::ModelColumn;
use super::*;
use crate::processing::document_model::{Block, PageInfo};
use crate::statements::testing::{t0, FixedEmbedder, TestClock, WordEmbedder};
use crate::statements::DynamicsStore;

async fn service(dir: &std::path::Path) -> (ResultService, Arc<TestClock>) {
    let clock = TestClock::at(t0());
    let ontology = Arc::new(shodh_ontology::Ontology::builtin_with_packs(&["research"]).unwrap());
    let dynamics = Arc::new(DynamicsStore::open(&dir.join("shodh.db"), None).unwrap());
    let store = StatementStore::open(
        &dir.join("lance"),
        crate::statements::testing::DIM,
        ontology,
        dynamics,
        Arc::new(FixedEmbedder(Arc::new(WordEmbedder::default()))),
        clock.clone(),
    )
    .await
    .unwrap();
    let db = Arc::new(ResearchDb::open(&dir.join("shodh.db"), None).unwrap());
    (ResultService::new(Arc::new(store), db, "test"), clock)
}

fn b(x0: f32, y0: f32, x1: f32, y1: f32) -> Option<BBox> {
    Some(BBox::new(x0, y0, x1, y1))
}

/// Table 2 of a fictional ANN paper: methods × (dataset, metric), with boxes.
fn ann_paper(hnsw_recall: &str) -> StructuredDocument {
    let table = Block::new(
        BlockKind::Table {
            header: vec![
                "Method".into(),
                "SIFT1M R@10 (%)".into(),
                "GIST1M R@10 (%)".into(),
            ],
            rows: vec![
                vec!["HNSW".into(), hnsw_recall.into(), "88.10*".into()],
                vec!["IVF-PQ".into(), "71.4 ± 0.3".into(), "-".into()],
            ],
            caption: Some("Table 2: Recall of graph and quantisation indexes.".into()),
            cell_coverage: None,
            cell_boxes: vec![
                vec![
                    b(100.0, 700.0, 150.0, 710.0),
                    b(160.0, 700.0, 220.0, 710.0),
                    b(230.0, 700.0, 290.0, 710.0),
                ],
                vec![
                    b(100.0, 688.0, 150.0, 698.0),
                    b(160.0, 688.0, 220.0, 698.0),
                    b(230.0, 688.0, 290.0, 698.0),
                ],
                vec![
                    b(100.0, 676.0, 150.0, 686.0),
                    b(160.0, 676.0, 220.0, 686.0),
                    b(230.0, 676.0, 290.0, 686.0),
                ],
            ],
        },
        "",
    )
    .on_page(6, b(100.0, 676.0, 290.0, 710.0));
    // Table 3 names its dataset only in the caption: its values wait for review.
    let ablation = Block::new(
        BlockKind::Table {
            header: vec!["Variant".into(), "Recall@100".into()],
            rows: vec![vec!["HNSW w/o pruning".into(), "97.20".into()]],
            caption: Some("Table 3: Ablation on SIFT1M.".into()),
            cell_coverage: None,
            cell_boxes: vec![
                vec![b(100.0, 500.0, 180.0, 510.0), b(190.0, 500.0, 240.0, 510.0)],
                vec![b(100.0, 488.0, 180.0, 498.0), b(190.0, 488.0, 240.0, 498.0)],
            ],
        },
        "",
    )
    .on_page(7, b(100.0, 488.0, 240.0, 510.0));
    StructuredDocument {
        pages: vec![
            PageInfo {
                number: 6,
                width: 612.0,
                height: 792.0,
            },
            PageInfo {
                number: 7,
                width: 612.0,
                height: 792.0,
            },
        ],
        blocks: vec![table, ablation],
    }
}

#[test]
fn canonical_ids_compare_labels_across_papers() {
    assert_eq!(canonical_id("metric", "R@10"), "metric:recall@10");
    assert_eq!(canonical_id("metric", "Recall@10 (%)"), "metric:recall@10");
    assert_eq!(canonical_id("metric", "Ppl."), "metric:perplexity");
    assert_eq!(
        canonical_id("dataset", "SIFT-1M"),
        canonical_id("dataset", "sift1m")
    );
    assert_eq!(
        canonical_id("method", "Gated DeltaNet"),
        "method:gateddeltanet"
    );
    assert_eq!(canonical_id("method", "GPT-3.5"), "method:gpt3.5");
}

#[tokio::test]
async fn extraction_stores_exact_values_with_cells_and_reviews_weak_ones() {
    let dir = tempfile::tempdir().unwrap();
    let (service, clock) = service(dir.path()).await;
    let path = "C:/papers/ann.pdf";
    let report = service
        .extract_document(path, &ann_paper("95.30"), Scope::Global, None)
        .await
        .unwrap();
    assert_eq!(report.tables, 2);
    assert_eq!(report.added, 4);
    assert_eq!(report.review, 1);
    assert!(report.skipped.is_empty());

    let listed = service.list(path).await.unwrap();
    let got: Vec<(&str, &str, &str, &str, &str, Option<&str>, u32)> = listed
        .results
        .iter()
        .map(|r| {
            (
                r.method.as_str(),
                r.dataset.as_str(),
                r.metric.as_str(),
                r.value.as_str(),
                r.value_text.as_str(),
                r.unit.as_deref(),
                r.page,
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("HNSW", "SIFT1M", "R@10", "95.30", "95.30", Some("%"), 6),
            ("HNSW", "GIST1M", "R@10", "88.10", "88.10*", Some("%"), 6),
            (
                "IVF-PQ",
                "SIFT1M",
                "R@10",
                "71.4",
                "71.4 ± 0.3",
                Some("%"),
                6
            ),
        ]
    );
    for r in &listed.results {
        // No invented values: the value occurs in the printed cell.
        assert!(r.value_text.contains(&r.value));
        assert_eq!(r.extractor, ExtractorKind::Rule);
        assert_eq!(r.metric_id, "metric:recall@10");
        assert_eq!(r.setting.as_deref(), Some("Table 2"));
    }
    let hnsw = &listed.results[0];
    assert_eq!(
        hnsw.region,
        Some(Region {
            page: 6,
            x0: 160.0,
            y0: 688.0,
            x1: 220.0,
            y1: 698.0
        })
    );
    assert_eq!(listed.review.len(), 1);
    let weak = &listed.review[0];
    assert_eq!(
        (weak.dataset.as_str(), weak.value.as_str()),
        ("SIFT1M", "97.20")
    );
    assert_eq!(weak.status, ResultStatus::Review);
    assert_eq!(listed.report.as_ref().map(|r| r.added), Some(4));

    // Extracting again changes nothing.
    clock.advance_days(1);
    let again = service
        .extract_document(path, &ann_paper("95.30"), Scope::Global, None)
        .await
        .unwrap();
    assert_eq!((again.added, again.unchanged), (0, 4));

    // Reviewing: accept the weak value, reject one rule value; neither comes back.
    service.review(&weak.id, true).await.unwrap();
    let ivf = listed
        .results
        .iter()
        .find(|r| r.method == "IVF-PQ")
        .unwrap();
    service.review(&ivf.id, false).await.unwrap();
    clock.advance_days(1);
    let third = service
        .extract_document(path, &ann_paper("95.30"), Scope::Global, None)
        .await
        .unwrap();
    assert_eq!(third.added, 0);
    let listed = service.list(path).await.unwrap();
    assert!(listed.review.is_empty());
    assert_eq!(listed.results.len(), 3);
    assert!(listed.results.iter().all(|r| r.method != "IVF-PQ"));
    let accepted = listed.results.iter().find(|r| r.value == "97.20").unwrap();
    assert_eq!(accepted.extractor, ExtractorKind::User);

    // A corrected parse replaces the rule value and keeps the user's.
    clock.advance_days(1);
    let fixed = service
        .extract_document(path, &ann_paper("95.31"), Scope::Global, None)
        .await
        .unwrap();
    assert_eq!(fixed.added, 1);
    let listed = service.list(path).await.unwrap();
    let values: Vec<&str> = listed.results.iter().map(|r| r.value.as_str()).collect();
    assert!(values.contains(&"95.31") && !values.contains(&"95.30"));
    assert!(values.contains(&"97.20"));
}

struct FixedModel(String);

#[async_trait::async_trait]
impl TextModel for FixedModel {
    fn model_id(&self) -> String {
        "test-model".to_string()
    }
    async fn complete(&self, prompt: &str, _max_tokens: usize) -> Result<String, String> {
        assert!(prompt.contains("Never write numbers"));
        assert!(prompt.contains("Dataset"));
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn a_model_names_header_roles_but_never_values() {
    let dir = tempfile::tempdir().unwrap();
    let (service, _) = service(dir.path()).await;
    let table = Block::new(
        BlockKind::Table {
            header: vec!["Configuration".into(), "Ppl.".into()],
            rows: vec![
                vec!["Linear attention".into(), "15.91".into()],
                vec!["TTT".into(), "15.23".into()],
            ],
            caption: Some("Table 1: Every model is trained and evaluated with PG19.".into()),
            cell_boxes: vec![],
            cell_coverage: None,
        },
        "",
    )
    .on_page(11, b(96.0, 640.0, 268.0, 702.0));
    let doc = StructuredDocument {
        pages: vec![PageInfo {
            number: 11,
            width: 612.0,
            height: 792.0,
        }],
        blocks: vec![table],
    };
    // Without a model the table is skipped with its reason.
    let report = service
        .extract_document("C:/papers/ttt.pdf", &doc, Scope::Global, None)
        .await
        .unwrap();
    assert_eq!(report.added, 0);
    assert!(report.skipped[0].reason.contains("no dataset"));
    // The model names "PG19" (in the caption, a name the rules do not take as one) and tries to add a value: only roles count.
    let model: Arc<dyn TextModel> = Arc::new(FixedModel(
        "Here: {\"columns\": [{\"index\": 1, \"metric\": \"Ppl.\", \"dataset\": \"PG19\", \"setting\": \"99.9\"}]}".into(),
    ));
    let report = service
        .extract_document("C:/papers/ttt.pdf", &doc, Scope::Global, Some(model))
        .await
        .unwrap();
    assert_eq!((report.added, report.review), (2, 2));
    assert_eq!(report.model.as_deref(), Some("test-model"));
    let listed = service.list("C:/papers/ttt.pdf").await.unwrap();
    assert!(listed.results.is_empty());
    let values: Vec<(&str, &str, &str, ExtractorKind)> = listed
        .review
        .iter()
        .map(|r| {
            (
                r.method.as_str(),
                r.dataset.as_str(),
                r.value.as_str(),
                r.extractor,
            )
        })
        .collect();
    assert_eq!(
        values,
        vec![
            ("Linear attention", "PG19", "15.91", ExtractorKind::Llm),
            ("TTT", "PG19", "15.23", ExtractorKind::Llm),
        ]
    );
    assert!(listed
        .review
        .iter()
        .all(|r| r.setting.as_deref() == Some("Table 1")));
}

#[tokio::test]
async fn comparisons_cite_cells_and_say_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let (service, _) = service(dir.path()).await;
    service
        .extract_document(
            "C:/papers/ann.pdf",
            &ann_paper("95.30"),
            Scope::Global,
            None,
        )
        .await
        .unwrap();
    // A second paper reporting the same metric on SIFT1M for another method.
    let other = Block::new(
        BlockKind::Table {
            header: vec!["Method".into(), "SIFT1M Recall@10".into()],
            rows: vec![
                vec!["DiskANN".into(), "96.0".into()],
                vec!["HNSW".into(), "95.1".into()],
            ],
            caption: Some("Table 1: Main results.".into()),
            cell_boxes: vec![],
            cell_coverage: None,
        },
        "",
    )
    .on_page(4, None);
    let doc = StructuredDocument {
        pages: vec![PageInfo {
            number: 4,
            width: 612.0,
            height: 792.0,
        }],
        blocks: vec![other],
    };
    service
        .extract_document("C:/papers/diskann.pdf", &doc, Scope::Global, None)
        .await
        .unwrap();
    // A third paper with an unrelated table.
    let unrelated = Block::new(
        BlockKind::Table {
            header: vec!["Model".into(), "C4 ppl".into()],
            rows: vec![vec!["Tiny".into(), "21.3".into()]],
            caption: None,
            cell_boxes: vec![],
            cell_coverage: None,
        },
        "",
    )
    .on_page(2, None);
    let doc = StructuredDocument {
        pages: vec![PageInfo {
            number: 2,
            width: 612.0,
            height: 792.0,
        }],
        blocks: vec![unrelated],
    };
    service
        .extract_document("C:/papers/lm.pdf", &doc, Scope::Global, None)
        .await
        .unwrap();

    let comparison = service
        .query(&ResultFilter {
            metric: Some("recall@10".into()),
            dataset: Some("SIFT1M".into()),
            known_papers: vec![
                "C:/papers/ann.pdf".into(),
                "C:/papers/diskann.pdf".into(),
                "C:/papers/lm.pdf".into(),
                "C:/papers/unread.pdf".into(),
            ],
            ..ResultFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(comparison.columns.len(), 1);
    let methods: Vec<&str> = comparison.rows.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(methods, vec!["DiskANN", "HNSW", "IVF-PQ"]);
    let hnsw = &comparison.rows[1].cells[&comparison.columns[0].key];
    let mut hnsw_values: Vec<(&str, &str)> = hnsw
        .iter()
        .map(|c| (c.file_name.as_str(), c.value_text.as_str()))
        .collect();
    hnsw_values.sort();
    assert_eq!(
        hnsw_values,
        vec![("ann.pdf", "95.30"), ("diskann.pdf", "95.1")]
    );
    assert_eq!(comparison.papers.len(), 2);
    assert_eq!(
        comparison.notes,
        vec![
            "2 papers report R@10 on SIFT1M.".to_string(),
            "1 paper in scope reports no comparable recall@10 on SIFT1M: lm.pdf.".to_string(),
            "1 paper in scope has not been scanned for results yet: unread.pdf.".to_string(),
        ]
    );
    // The value waiting for review is counted, not used.
    let recall100 = service
        .query(&ResultFilter {
            metric: Some("Recall@100".into()),
            ..ResultFilter::default()
        })
        .await
        .unwrap();
    assert!(recall100.rows.is_empty());
    assert_eq!(recall100.pending_review, 1);
    assert!(recall100.notes[0].starts_with("No accepted results match Recall@100"));
    assert!(recall100
        .notes
        .iter()
        .any(|n| n == "1 matching value awaits review and is not included."));

    let facets = service.facets(&[]).await.unwrap();
    assert_eq!(facets.metrics[0].id, "metric:recall@10");
    assert_eq!(facets.papers.len(), 3);
}

// ── The research-paper corpus ───────────────────────────────────────────────
//
// `fixtures/corpus_tables.json` is the extractor input for every table block of the public
// arXiv papers in the user's corpus, parsed with the table model on table-candidate pages
// (see `processing::table_corpus`; the user's own papers and non-arXiv files are never
// included). The model's output depends on the CPU's kernels, so it is checked in rather
// than produced in CI. These tests pin what the rules do on real tables: nothing is ever
// invented, every refusal has its reason, and the values, settings, pages and cell boxes
// are exactly the parser's.

fn corpus() -> Vec<(String, TableInput)> {
    let text = include_str!("../fixtures/corpus_tables.json");
    let papers: BTreeMap<String, Vec<TableInput>> = serde_json::from_str(text).unwrap();
    papers
        .into_iter()
        .flat_map(|(name, tables)| tables.into_iter().map(move |t| (name.clone(), t)))
        .collect()
}

fn corpus_table(paper: &str, page: u32, index: usize) -> TableInput {
    corpus()
        .into_iter()
        .filter(|(name, t)| name.starts_with(paper) && t.page == page)
        .map(|(_, t)| t)
        .nth(index)
        .unwrap()
}

#[test]
fn corpus_fixture_holds_only_public_arxiv_papers() {
    for (name, _) in corpus() {
        let arxiv = name.len() > 10
            && name[..4].chars().all(|c| c.is_ascii_digit())
            && &name[4..5] == "."
            && name[5..10].chars().all(|c| c.is_ascii_digit());
        assert!(
            arxiv || name.starts_with("KAN - Kolmogorov-Arnold Networks"),
            "{name}"
        );
    }
}

#[test]
fn corpus_tables_never_yield_an_invented_value() {
    let tables = corpus();
    assert_eq!(tables.len(), 54);
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut candidates = Vec::new();
    for (name, table) in &tables {
        let reading = read_table(table, None);
        let width = std::iter::once(table.header.len())
            .chain(table.rows.iter().map(Vec::len))
            .max()
            .unwrap_or(0);
        for c in &reading.candidates {
            // Every value is a substring of a real body cell at its row and column.
            let body_cells: Vec<&String> =
                table.rows.iter().filter_map(|r| r.get(c.column)).collect();
            assert!(c.column < width, "{name}");
            assert!(
                body_cells.iter().any(|cell| cell.trim() == c.cell_text),
                "{name}: {c:?}"
            );
            assert!(c.cell_text.contains(&c.number.lexeme), "{name}: {c:?}");
            assert_eq!(c.page, table.page);
        }
        candidates.extend(reading.candidates);
        if let Some(reason) = reading.skipped {
            *reasons.entry(reason).or_default() += 1;
        }
    }
    // What extraction stores: ambiguous duplicates (the same method, dataset, metric and
    // setting with different values, e.g. one model trained on two corpora told apart only
    // by a rotated label) are dropped.
    let stored = candidates.len() - ambiguous(candidates.iter()).len();
    assert_eq!((candidates.len(), stored), (820, 770));
    let expected: BTreeMap<String, usize> = [
        (
            "no dataset or metric is named in the column headers or the caption",
            30,
        ),
        ("no column holds numbers", 9),
        (
            "the column headers are numbers: the table's header was not recovered",
            1,
        ),
        ("no body rows hold numbers", 1),
        ("no column holds method names", 2),
    ]
    .into_iter()
    .map(|(r, n)| (r.to_string(), n))
    .collect();
    assert_eq!(reasons, expected);
}

#[test]
fn corpus_model_labels_not_in_the_table_text_are_refused() {
    // Table 1 of the TTT paper names no dataset anywhere in the table or its caption (the
    // training data is described in Subsection 3.1), so a model saying "Books" is refused.
    let ttt = corpus_table("2407.04620", 11, 0);
    let roles = HeaderRoles {
        columns: vec![ModelColumn {
            index: 1,
            metric: Some("Ppl.".into()),
            dataset: Some("Books".into()),
            setting: None,
        }],
    };
    let (verified, refused) = verify_roles(&ttt, roles, 3);
    assert_eq!(refused, 1);
    assert!(read_table(&ttt, Some(&verified)).candidates.is_empty());
}

#[test]
fn corpus_cells_give_exact_values_pages_and_boxes() {
    let cell = |x0: f32, y0: f32, x1: f32, y1: f32| Some(BBox::new(x0, y0, x1, y1).rounded());

    // DeltaNet Table 1: dataset and metric in each header, the model size and token count
    // from the group rows, values with their cell boxes.
    let deltanet = corpus_table("2406.06484", 8, 0);
    let reading = read_table(&deltanet, None);
    let first = &reading.candidates[0];
    assert_eq!(
        (
            first.method.as_str(),
            first.dataset.as_str(),
            first.metric.as_str(),
            first.cell_text.as_str(),
            first.setting.as_str(),
            first.confidence,
        ),
        (
            "Transformer++",
            "Wiki.",
            "ppl",
            "28.39",
            "Table 1; 340M params / 15B tokens",
            0.9
        )
    );
    assert!(first.cell_box.is_some());
    // A label printed as a variant of the row above names its method in full.
    assert!(reading
        .candidates
        .iter()
        .any(|c| c.method == "GLA (w. conv)" && c.cell_text == "29.47"));
    assert!(reading
        .candidates
        .iter()
        .any(|c| c.method == "DeltaNet + Sliding Attn" && c.cell_text == "27.06"));
    // The derived average column is not a result.
    assert!(reading.candidates.iter().all(|c| c.column != 9));

    // TTT Table 1 with a caption that names the dataset: the model's cell boxes.
    let mut ttt = corpus_table("2407.04620", 11, 0);
    ttt.caption = Some("Table 1. Ablations on Books.".into());
    let reading = read_table(&ttt, None);
    let got: Vec<(String, String, Option<BBox>, u32)> = reading
        .candidates
        .iter()
        .filter(|c| c.metric == "Ppl.")
        .take(2)
        .map(|c| {
            (
                c.method.clone(),
                c.number.decimal.clone(),
                c.cell_box,
                c.page,
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "Linear attention [44]".into(),
                "15.91".into(),
                cell(205.7, 670.7, 228.1, 679.7),
                11
            ),
            (
                "Linear attn. improved".into(),
                "15.23".into(),
                cell(205.7, 654.2, 228.1, 663.2),
                11
            ),
        ]
    );

    // MIRAS Table 3: dataset headers spanning a row of context lengths.
    let niah = corpus_table("2504.13173", 18, 0);
    let reading = read_table(&niah, None);
    let mamba: Vec<(&str, &str, &str)> = reading
        .candidates
        .iter()
        .filter(|c| c.method == "Mamba2")
        .take(4)
        .map(|c| (c.dataset.as_str(), c.setting.as_str(), c.cell_text.as_str()))
        .collect();
    assert_eq!(
        mamba,
        vec![
            ("S-NIAH-PK", "Table 3; 2K", "98.6"),
            ("S-NIAH-PK", "Table 3; 4K", "61.4"),
            ("S-NIAH-PK", "Table 3; 8K", "31.0"),
            ("S-NIAH-N", "Table 3; 2K", "98.4"),
        ]
    );

    // PASCAL Table 5: two tables side by side, each with a dataset column.
    let pascal = corpus_table("2505.01730", 12, 0);
    let reading = read_table(&pascal, None);
    let seenn: Vec<(&str, &str, &str)> = reading
        .candidates
        .iter()
        .filter(|c| c.method.contains("SEENN-1") && c.metric == "Acc.")
        .map(|c| (c.method.as_str(), c.dataset.as_str(), c.cell_text.as_str()))
        .collect();
    assert_eq!(
        seenn,
        vec![
            ("ResNet-34 SEENN-1", "ImageNet", "71.84%"),
            ("ResNet-18 SEENN-1", "CIFAR-10", "95.08%"),
            ("ResNet-18 SEENN-1", "CIFAR-100", "65.48%"),
        ]
    );
}
