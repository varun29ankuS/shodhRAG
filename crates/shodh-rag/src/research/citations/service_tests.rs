//! Graph builds against a real statement store (LanceDB + SQLite in a temporary folder),
//! with scans made in the test and OpenAlex answers replayed from recorded JSON.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::identity::LocalIdentity;
use super::reference::parse_reference;
use super::resolve::tests::{fixture, Replay};
use super::resolve::{Resolver, WorkRequest};
use super::scan::{PaperScan, ScannedReference, SCAN_VERSION};
use super::service::{CitationService, GraphSlot, ScanCounts};
use crate::research::db::ResearchDb;
use crate::research::results::ResultService;
use crate::statements::testing::{t0, FixedEmbedder, TestClock, WordEmbedder};
use crate::statements::{DynamicsStore, StatementQuery, StatementStore};

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<StatementStore>,
    db: Arc<ResearchDb>,
    results: ResultService,
}

async fn fixture_store() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ontology = Arc::new(shodh_ontology::Ontology::builtin_with_packs(&["research"]).unwrap());
    let dynamics = Arc::new(DynamicsStore::open(&dir.path().join("shodh.db"), None).unwrap());
    let store = Arc::new(
        StatementStore::open(
            &dir.path().join("lance"),
            crate::statements::testing::DIM,
            ontology,
            dynamics,
            Arc::new(FixedEmbedder(Arc::new(WordEmbedder::default()))),
            TestClock::at(t0()),
        )
        .await
        .unwrap(),
    );
    let db = Arc::new(ResearchDb::open(&dir.path().join("shodh.db"), None).unwrap());
    let results = ResultService::new(store.clone(), db.clone(), "test");
    Fixture {
        _dir: dir,
        store,
        db,
        results,
    }
}

fn scan(path: &str, title: &str, arxiv: Option<&str>, refs: &[&str]) -> PaperScan {
    PaperScan {
        version: SCAN_VERSION.into(),
        file_path: path.into(),
        identity: LocalIdentity {
            title: Some(title.into()),
            arxiv_id: arxiv.map(str::to_string),
            ..LocalIdentity::default()
        },
        references: refs
            .iter()
            .enumerate()
            .map(|(index, text)| ScannedReference {
                index,
                page: Some(9),
                regions: Vec::new(),
                parsed: parse_reference(text),
            })
            .collect(),
        rejected: Vec::new(),
        truncated: 0,
        headings: Vec::new(),
        captions: Vec::new(),
        pages: 10,
    }
}

const LSTM: &str = "Hochreiter, S. and Schmidhuber, J. Long short-term memory. Neural Computation, 9(8):1735–1780, 1997. doi:10.1162/neco.1997.9.8.1735";
const FWP: &str = "Schlag, I., Irie, K., and Schmidhuber, J. Linear transformers are secretly fast weight programmers. In ICML, 2021.";

fn library() -> Vec<PaperScan> {
    vec![
        scan(
            "C:/p/delta.pdf",
            "Delta rule transformers",
            Some("2406.06484"),
            &[LSTM, FWP],
        ),
        scan(
            "C:/p/fwp.pdf",
            "Linear transformers are secretly fast weight programmers",
            None,
            &[LSTM],
        ),
    ]
}

fn nothing(_: super::service::BuildProgress) {}

async fn graph_statements(store: &StatementStore) -> Vec<crate::statements::StoredStatement> {
    store
        .query(&StatementQuery {
            classes: vec![
                "Paper".into(),
                "Author".into(),
                "Venue".into(),
                "Method".into(),
            ],
            limit: Some(500),
            ..StatementQuery::default()
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn a_local_build_stores_the_graph_and_a_rebuild_writes_nothing() {
    let f = fixture_store().await;
    let service = CitationService::new(f.store.clone(), f.db.clone(), GraphSlot::default());
    let offline = Resolver::cache_only(f.db.clone());
    let report = service
        .build_from_scans(
            library(),
            ScanCounts::default(),
            &f.results,
            &offline,
            &nothing,
        )
        .await
        .unwrap();
    assert!(!report.online);
    assert_eq!(
        report.lookups.map(|l| l.requests),
        Some(0),
        "local-only builds send nothing"
    );
    assert_eq!(report.size.library_papers, 2);
    assert_eq!(report.size.papers, 3, "LSTM once, FWP is the library paper");
    assert_eq!(report.size.cites, 3);
    assert_eq!(report.references_linked_to_library, 1);
    assert!(report.added >= 3);
    assert_eq!(report.updated + report.removed + report.refused, 0);

    let graph = service.graph(&f.results).await.unwrap();
    let delta = graph.paper("paper:arxiv:2406.06484").unwrap();
    assert!(delta.in_library && delta.statement_id.is_some());
    let fwp = graph.paper_for_file("C:/p/fwp.pdf").unwrap();
    assert_eq!(graph.citers(&fwp.id).len(), 1);
    assert!(graph
        .evidence(&delta.id, &fwp.id)
        .unwrap()
        .text
        .contains("fast weight"));

    let again = service
        .build_from_scans(
            library(),
            ScanCounts::default(),
            &f.results,
            &offline,
            &nothing,
        )
        .await
        .unwrap();
    assert_eq!((again.added, again.updated, again.removed), (0, 0, 0));
    assert_eq!(again.unchanged, report.added);

    // The graph reads back from the store in a new session, evidence included.
    let fresh = CitationService::new(f.store.clone(), f.db.clone(), GraphSlot::default());
    let loaded = fresh.graph(&f.results).await.unwrap();
    assert_eq!(loaded.size().papers, 3);
    assert_eq!(loaded.size().cites, 3);
    assert!(loaded.evidence(&delta.id, &fwp.id).is_some());
}

#[tokio::test]
async fn a_dropped_reference_supersedes_the_paper_and_forgets_the_orphan() {
    let f = fixture_store().await;
    let service = CitationService::new(f.store.clone(), f.db.clone(), GraphSlot::default());
    let offline = Resolver::cache_only(f.db.clone());
    service
        .build_from_scans(
            library(),
            ScanCounts::default(),
            &f.results,
            &offline,
            &nothing,
        )
        .await
        .unwrap();
    let mut changed = library();
    changed[0].references.remove(0);
    changed[1].references.clear();
    let report = service
        .build_from_scans(
            changed,
            ScanCounts::default(),
            &f.results,
            &offline,
            &nothing,
        )
        .await
        .unwrap();
    assert_eq!(report.updated, 2, "both library papers changed their cites");
    assert!(report.removed >= 1, "LSTM is no longer cited");
    assert_eq!(report.size.papers, 2);
    let stored = graph_statements(&f.store).await;
    assert!(!stored.iter().any(|s| s
        .statement
        .subject
        .as_ref()
        .is_some_and(|e| e.id.contains("neco"))));
    let delta = stored
        .iter()
        .find(|s| {
            s.statement
                .subject
                .as_ref()
                .is_some_and(|e| e.id == "paper:arxiv:2406.06484")
        })
        .unwrap();
    // History keeps the old version.
    let history = f.store.history_ids(delta.id()).await.unwrap();
    assert_eq!(history.len(), 2);
}

#[tokio::test]
async fn online_builds_match_by_identifier_once_and_reuse_the_cache() {
    let f = fixture_store().await;
    let service = CitationService::new(f.store.clone(), f.db.clone(), GraphSlot::default());
    let lstm = WorkRequest::Doi("10.1162/neco.1997.9.8.1735".into());
    let replay = Arc::new(Replay::new(vec![(lstm, 200, fixture("work_by_doi"))]));
    let online = Resolver::online(replay.clone(), f.db.clone()).with_limits(50, Duration::ZERO);
    let report = service
        .build_from_scans(
            library(),
            ScanCounts::default(),
            &f.results,
            &online,
            &nothing,
        )
        .await
        .unwrap();
    assert!(report.online);
    assert_eq!(report.references_resolved, 2, "LSTM in both papers");
    let graph = service.graph(&f.results).await.unwrap();
    let node = graph.paper("paper:doi:10.1162/neco.1997.9.8.1735").unwrap();
    assert_eq!(node.openalex_id.as_deref(), Some("W2064675550"));
    assert!(node.cited_by_count.unwrap_or(0) > 1000);
    let first = replay.requests.load(Ordering::SeqCst);
    assert!(
        first >= 2,
        "the library paper's arXiv id and the DOI were asked"
    );

    let online = Resolver::online(replay.clone(), f.db.clone()).with_limits(50, Duration::ZERO);
    let again = service
        .build_from_scans(
            library(),
            ScanCounts::default(),
            &f.results,
            &online,
            &nothing,
        )
        .await
        .unwrap();
    assert_eq!(
        replay.requests.load(Ordering::SeqCst),
        first,
        "every answer came from the cache"
    );
    assert_eq!((again.added, again.updated), (0, 0));
}
