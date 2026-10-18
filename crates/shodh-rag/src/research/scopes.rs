//! Where research statements are visible. A snippet or result made in the Library belongs
//! to no workspace (the Library has none), so it is global and every workspace run over
//! its file sees it; one an assistant made in a workspace chat belongs to that workspace.
//! Older versions filed what the Library made under the id of the Library source the file
//! was in, an id no workspace run looks for; [`settle_scopes`] moves such rows, and the
//! rows of workspaces that no longer exist, to [`Scope::Global`].

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::Serialize;
use serde_json::json;

use super::results::RESULT_CLASS;
use super::snippets::SNIPPET_CLASS;
use crate::audit::{AuditEventType, AuditRecord, SYSTEM_PRINCIPAL};
use crate::statements::{Scope, StatementResult, StatementStore};

/// Label of the extractor of the snippets the user saved (`ExtractorKind::User`).
const USER_EXTRACTOR: &str = "user";

/// Ontology source of the research pack: its classes other than snippets and results
/// (the citation graph's papers, authors, venues, methods) count as "others".
const RESEARCH_SOURCE: &str = "shodh.research";

/// Statements moved, by kind (every version counts).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MovedCounts {
    pub memories: u64,
    pub snippets: u64,
    pub results: u64,
    pub others: u64,
}

impl MovedCounts {
    /// Statements moved.
    pub fn total(&self) -> u64 {
        self.memories + self.snippets + self.results + self.others
    }

    fn add(&mut self, store: &StatementStore, by_class: &BTreeMap<String, u64>) {
        for (class, n) in by_class {
            let slot = if class == SNIPPET_CLASS {
                &mut self.snippets
            } else if class == RESULT_CLASS {
                &mut self.results
            } else if store
                .ontology()
                .class(class)
                .is_some_and(|c| c.source == RESEARCH_SOURCE)
            {
                &mut self.others
            } else {
                &mut self.memories
            };
            *slot += n;
        }
    }
}

/// Why statements moved to [`Scope::Global`]; recorded in the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveReason {
    /// The one-time move of what the Library made under a workspace scope.
    LibraryMigration,
    /// The user deleted the workspace.
    WorkspaceDeleted,
    /// The scope names no workspace.
    OrphanSweep,
}

impl MoveReason {
    pub fn as_str(self) -> &'static str {
        match self {
            MoveReason::LibraryMigration => "library-scope migration",
            MoveReason::WorkspaceDeleted => "workspace deleted",
            MoveReason::OrphanSweep => "orphan scope sweep",
        }
    }
}

/// Statements moved to [`Scope::Global`] from the scopes `from`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScopeMove {
    /// The scopes something moved from.
    pub from: BTreeSet<Scope>,
    pub moved: MovedCounts,
}

impl ScopeMove {
    fn add(&mut self, store: &StatementStore, scope: &Scope, by_class: &BTreeMap<String, u64>) {
        if by_class.values().any(|n| *n > 0) {
            self.from.insert(scope.clone());
            self.moved.add(store, by_class);
        }
    }

    /// The `scope_change` audit event of this move: the scopes, the counts per kind and
    /// why, never statement text. `None` when nothing moved. A deleted workspace is the
    /// user's doing; the other moves are the app's.
    pub fn audit_record(&self, reason: MoveReason) -> Option<AuditRecord> {
        let total = self.moved.total();
        if total == 0 {
            return None;
        }
        let record = AuditRecord::new(
            AuditEventType::ScopeChange,
            json!({
                "reason": reason.as_str(),
                "from": self.from.iter().map(Scope::as_key).collect::<Vec<_>>(),
                "to": Scope::Global.as_key(),
                "moved": self.moved,
                "total": total,
            }),
        );
        Some(match reason {
            MoveReason::WorkspaceDeleted => record,
            MoveReason::LibraryMigration | MoveReason::OrphanSweep => {
                record.principal(SYSTEM_PRINCIPAL)
            }
        })
    }
}

/// What [`settle_scopes`] moved to [`Scope::Global`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SettledScopes {
    /// Results, and snippets the user saved, that were filed under a workspace scope.
    pub library: ScopeMove,
    /// Statements of scopes that name no workspace.
    pub orphaned: ScopeMove,
    /// Nothing was moved: no workspace is listed, yet statements name workspaces.
    pub sweep_skipped: bool,
}

impl SettledScopes {
    /// Statements moved.
    pub fn moved(&self) -> u64 {
        self.library.moved.total() + self.orphaned.moved.total()
    }

    /// One audit event per kind of move that moved something.
    pub fn audit_records(&self) -> Vec<AuditRecord> {
        [
            self.library.audit_record(MoveReason::LibraryMigration),
            self.orphaned.audit_record(MoveReason::OrphanSweep),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// Moves every statement of `scope` (a workspace being deleted) to [`Scope::Global`].
pub async fn move_to_global(store: &StatementStore, scope: &Scope) -> StatementResult<ScopeMove> {
    let mut out = ScopeMove::default();
    let by_class = store.rescope(scope, &Scope::Global, &[], None).await?;
    out.add(store, scope, &by_class);
    Ok(out)
}

/// The prefix of the ids [`crate::workspaces::WorkspaceStore::create`] gives workspaces.
const CREATED_WORKSPACE_PREFIX: &str = "ws-";

/// Moves statements whose scope no run can see to [`Scope::Global`]:
/// - with `library`, every result and every snippet the user saved that is filed under a
///   workspace scope. Only the Library makes them (results are extracted there; the user
///   saves snippets in its viewer and in answers' viewers, which file them globally), and
///   older versions filed them under a Library source id. Run it once: from then on
///   nothing files them under a workspace;
/// - the statements (memories, snippets, results) of each scope in `scopes` whose id is
///   not in `workspaces` (every workspace, archived ones too): a Library source id, or a
///   workspace deleted before deleting one moved them.
///
/// `scopes` must be read ([`StatementStore::scopes`]) before `workspaces`: a workspace
/// created after the scopes were read is then never mistaken for a missing one. Every
/// step is idempotent. A failure stops the run and is returned next to what was moved
/// before it, so every completed move can still be recorded; a later call completes the
/// rest.
///
/// No workspace listed while statements name ids only the workspace store makes
/// (`ws-…`) means the list is not what it seems (an emptied or replaced database): the
/// sweep is then skipped ([`SettledScopes::sweep_skipped`]) rather than making every
/// workspace's memories global.
pub async fn settle_scopes(
    store: &StatementStore,
    scopes: &BTreeSet<Scope>,
    workspaces: &HashSet<String>,
    library: bool,
) -> (SettledScopes, StatementResult<()>) {
    let mut settled = SettledScopes::default();
    let created = scopes
        .iter()
        .any(|s| matches!(s, Scope::Workspace(id) if id.starts_with(CREATED_WORKSPACE_PREFIX)));
    if workspaces.is_empty() && created {
        tracing::warn!(
            target: "shodh::workspaces",
            "no workspace is listed but statements belong to workspaces: scopes left as they are"
        );
        settled.sweep_skipped = true;
        return (settled, Ok(()));
    }
    let outcome = settle_into(store, scopes, workspaces, library, &mut settled).await;
    (settled, outcome)
}

async fn settle_into(
    store: &StatementStore,
    scopes: &BTreeSet<Scope>,
    workspaces: &HashSet<String>,
    library: bool,
    settled: &mut SettledScopes,
) -> StatementResult<()> {
    for scope in scopes {
        let Scope::Workspace(id) = scope else {
            continue;
        };
        if !workspaces.contains(id) {
            let by_class = store.rescope(scope, &Scope::Global, &[], None).await?;
            settled.orphaned.add(store, scope, &by_class);
            continue;
        }
        if library {
            let results = store
                .rescope(scope, &Scope::Global, &[RESULT_CLASS], None)
                .await?;
            settled.library.add(store, scope, &results);
            let snippets = store
                .rescope(
                    scope,
                    &Scope::Global,
                    &[SNIPPET_CLASS],
                    Some(USER_EXTRACTOR),
                )
                .await?;
            settled.library.add(store, scope, &snippets);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::research::db::ResearchDb;
    use crate::research::pdf_text::PageRect;
    use crate::research::snippets::{NewSnippet, SnippetAuthor, SnippetKind, SnippetService};
    use crate::statements::testing::{note, t0, FixedEmbedder, TestClock, WordEmbedder, DIM};
    use crate::statements::{DynamicsStore, PutIntent, StatementQuery};

    struct Stores {
        dir: tempfile::TempDir,
        store: Arc<StatementStore>,
        snippets: SnippetService,
        clock: Arc<TestClock>,
    }

    async fn stores() -> Stores {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::at(t0());
        let ontology =
            Arc::new(shodh_ontology::Ontology::builtin_with_packs(&["research"]).unwrap());
        let dynamics = Arc::new(DynamicsStore::open(&dir.path().join("shodh.db"), None).unwrap());
        let store = Arc::new(
            StatementStore::open(
                &dir.path().join("lance"),
                DIM,
                ontology,
                dynamics,
                Arc::new(FixedEmbedder(Arc::new(WordEmbedder::default()))),
                clock.clone(),
            )
            .await
            .unwrap(),
        );
        let db = Arc::new(ResearchDb::open(&dir.path().join("shodh.db"), None).unwrap());
        Stores {
            snippets: SnippetService::new(store.clone(), db, "test"),
            store,
            dir,
            clock,
        }
    }

    fn snippet(path: &str, text: &str, scope: Scope, author: SnippetAuthor) -> NewSnippet {
        NewSnippet {
            file_path: path.to_string(),
            page: 3,
            rect: PageRect {
                x: 10.0,
                y: 20.0,
                width: 200.0,
                height: 40.0,
            },
            text: text.to_string(),
            image_png: None,
            title: None,
            note: None,
            tags: Vec::new(),
            kind: SnippetKind::Passage,
            scope,
            author,
        }
    }

    /// [`settle_scopes`] that must succeed.
    async fn settle(
        store: &StatementStore,
        scopes: &BTreeSet<Scope>,
        workspaces: &HashSet<String>,
        library: bool,
    ) -> SettledScopes {
        let (settled, outcome) = settle_scopes(store, scopes, workspaces, library).await;
        outcome.unwrap();
        settled
    }

    async fn scope_of(store: &StatementStore, id: &str) -> Scope {
        store.get(id).await.unwrap().scope
    }

    /// Library-made snippets filed under a Library source id become global, with every
    /// version and their dynamics; an assistant's snippet of a live workspace stays; the
    /// memories of a scope that names no workspace become global; nothing moves twice.
    #[tokio::test]
    async fn library_research_and_orphaned_scopes_become_global() {
        let s = stores().await;
        let source = Scope::Workspace("src-papers".into());
        let live = Scope::Workspace("ws-thesis".into());
        let saved = s
            .snippets
            .create(snippet(
                "C:/papers/ann.pdf",
                "Graph indexes reach a recall of 95 percent on SIFT1M at ten neighbours.",
                source.clone(),
                SnippetAuthor::User,
            ))
            .await
            .unwrap();
        // A revision, a day later (at the same instant both versions would be current):
        // the history moves with the snippet.
        s.clock.advance_days(1);
        let revised = s
            .snippets
            .update(
                &saved.id,
                crate::research::snippets::SnippetPatch {
                    title: Some("Recall".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_ne!(revised.statement_id, saved.statement_id);
        let by_agent = s
            .snippets
            .create(snippet(
                "C:/papers/ann.pdf",
                "Quantisation trades recall for memory: IVF-PQ keeps 71 percent at ten.",
                live.clone(),
                SnippetAuthor::Agent { model: "m".into() },
            ))
            .await
            .unwrap();
        // The user's own snippet in a live workspace (a legacy workspace keeps the Library
        // source id it was made from) is Library-made too.
        let legacy = s
            .snippets
            .create(snippet(
                "C:/papers/lm.pdf",
                "Perplexity falls with longer training on the deduplicated corpus.",
                live.clone(),
                SnippetAuthor::User,
            ))
            .await
            .unwrap();
        let gone = Scope::Workspace("ws-deleted".into());
        s.store
            .put(
                note("m1", "Prefers recall at ten for comparisons", t0()),
                gone.clone(),
                PutIntent::Auto,
            )
            .await
            .unwrap();
        s.store
            .put(
                note("m2", "Writes the thesis in LaTeX", t0()),
                live.clone(),
                PutIntent::Auto,
            )
            .await
            .unwrap();

        let scopes = s.store.scopes().await.unwrap();
        assert_eq!(
            scopes,
            BTreeSet::from([source.clone(), live.clone(), gone.clone()])
        );
        let workspaces = HashSet::from(["ws-thesis".to_string()]);
        // No workspace listed, yet statements name created workspaces: nothing moves.
        let skipped = settle(&s.store, &scopes, &HashSet::new(), true).await;
        assert!(skipped.sweep_skipped);
        assert_eq!(skipped.moved(), 0);
        assert!(skipped.audit_records().is_empty());
        assert_eq!(scope_of(&s.store, "m2").await, live);
        assert_eq!(s.store.scopes().await.unwrap(), scopes);

        let settled = settle(&s.store, &scopes, &workspaces, true).await;
        assert!(!settled.sweep_skipped);
        // Both versions of the source-filed snippet, and the memory: orphaned; the
        // user's snippet in the live workspace: Library-made.
        assert_eq!(
            settled.library,
            ScopeMove {
                from: BTreeSet::from([live.clone()]),
                moved: MovedCounts {
                    snippets: 1,
                    ..MovedCounts::default()
                },
            }
        );
        assert_eq!(
            settled.orphaned,
            ScopeMove {
                from: BTreeSet::from([source.clone(), gone.clone()]),
                moved: MovedCounts {
                    memories: 1,
                    snippets: 2,
                    ..MovedCounts::default()
                },
            }
        );
        // Each move is one audit event of scopes and counts, without any statement text.
        let records = settled.audit_records();
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(
            |r| r.event_type == crate::audit::AuditEventType::ScopeChange
                && r.principal == crate::audit::SYSTEM_PRINCIPAL
        ));
        assert_eq!(
            records[1].payload,
            serde_json::json!({
                "reason": "orphan scope sweep",
                "from": ["workspace:src-papers", "workspace:ws-deleted"],
                "to": "global",
                "moved": {"memories": 1, "snippets": 2, "results": 0, "others": 0},
                "total": 3,
            })
        );
        assert_eq!(records[0].payload["reason"], "library-scope migration");
        let text: String = records.iter().map(|r| r.payload.to_string()).collect();
        assert!(
            !text.contains("recall") && !text.contains("LaTeX"),
            "{text}"
        );
        assert_eq!(
            scope_of(&s.store, &revised.statement_id).await,
            Scope::Global
        );
        assert_eq!(
            scope_of(&s.store, &legacy.statement_id).await,
            Scope::Global
        );
        assert_eq!(scope_of(&s.store, "m1").await, Scope::Global);
        assert_eq!(scope_of(&s.store, &by_agent.statement_id).await, live);
        assert_eq!(scope_of(&s.store, "m2").await, live);
        let history = s
            .store
            .query_all(&StatementQuery {
                classes: vec![SNIPPET_CLASS.to_string()],
                include_history: true,
                scopes: vec![Scope::Global],
                ..StatementQuery::default()
            })
            .await
            .unwrap();
        let ids: BTreeSet<&str> = history.iter().map(|h| h.id()).collect();
        assert!(ids.contains(saved.statement_id.as_str()), "{ids:?}");
        assert!(ids.contains(revised.statement_id.as_str()), "{ids:?}");
        // The move keeps the supersede chain: the old version is still history.
        let old = s.store.get(&saved.statement_id).await.unwrap();
        assert_eq!(
            old.superseded_by.as_deref(),
            Some(revised.statement_id.as_str())
        );
        assert!(!old.is_current_at(s.store.now()));
        // A global snippet is listed from the workspace that holds its file.
        let listed = s
            .snippets
            .list(&crate::research::snippets::SnippetQuery {
                file_path: Some("C:/papers/ann.pdf".into()),
                scopes: live.visible(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(listed.len(), 2, "{listed:?}");
        // The dynamics record the new scope.
        let conn = rusqlite::Connection::open(s.dir.path().join("shodh.db")).unwrap();
        let recorded: String = conn
            .query_row(
                "SELECT scope FROM statement_dynamics WHERE statement_id = ?1",
                rusqlite::params![revised.statement_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(recorded, "global");
        drop(conn);

        // Settled: nothing moves again, and without `library` a live workspace's
        // statements stay whatever they are.
        let again = settle(
            &s.store,
            &s.store.scopes().await.unwrap(),
            &workspaces,
            true,
        )
        .await;
        assert_eq!(again.moved(), 0);
        assert!(again.audit_records().is_empty());
        let kept = s
            .snippets
            .create(snippet(
                "C:/papers/lm.pdf",
                "A workspace snippet the user saved after the move stays in its workspace.",
                live.clone(),
                SnippetAuthor::User,
            ))
            .await
            .unwrap();
        settle(
            &s.store,
            &s.store.scopes().await.unwrap(),
            &workspaces,
            false,
        )
        .await;
        assert_eq!(scope_of(&s.store, &kept.statement_id).await, live);
    }
}
