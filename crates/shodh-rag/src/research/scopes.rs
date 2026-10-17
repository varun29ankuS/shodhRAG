//! Where research statements are visible. A snippet or result made in the Library belongs
//! to no workspace (the Library has none), so it is global and every workspace run over
//! its file sees it; one an assistant made in a workspace chat belongs to that workspace.
//! Older versions filed what the Library made under the id of the Library source the file
//! was in, an id no workspace run looks for; [`settle_scopes`] moves such rows, and the
//! rows of workspaces that no longer exist, to [`Scope::Global`].

use std::collections::{BTreeSet, HashSet};

use super::results::RESULT_CLASS;
use super::snippets::SNIPPET_CLASS;
use crate::statements::{Scope, StatementResult, StatementStore};

/// Label of the extractor of the snippets the user saved (`ExtractorKind::User`).
const USER_EXTRACTOR: &str = "user";

/// What [`settle_scopes`] moved to [`Scope::Global`], in statements (every version).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SettledScopes {
    /// Results, and snippets the user saved, that were filed under a workspace scope.
    pub library: u64,
    /// Statements of scopes that name no workspace.
    pub orphaned: u64,
}

impl SettledScopes {
    /// Statements moved.
    pub fn moved(&self) -> u64 {
        self.library + self.orphaned
    }
}

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
/// step is idempotent; a failure leaves what was moved moved, and a later call completes
/// the rest.
pub async fn settle_scopes(
    store: &StatementStore,
    scopes: &BTreeSet<Scope>,
    workspaces: &HashSet<String>,
    library: bool,
) -> StatementResult<SettledScopes> {
    let mut settled = SettledScopes::default();
    for scope in scopes {
        let Scope::Workspace(id) = scope else {
            continue;
        };
        if !workspaces.contains(id) {
            settled.orphaned += store.rescope(scope, &Scope::Global, &[], None).await?;
            continue;
        }
        if library {
            settled.library += store
                .rescope(scope, &Scope::Global, &[RESULT_CLASS], None)
                .await?;
            settled.library += store
                .rescope(
                    scope,
                    &Scope::Global,
                    &[SNIPPET_CLASS],
                    Some(USER_EXTRACTOR),
                )
                .await?;
        }
    }
    Ok(settled)
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
    }

    async fn stores() -> Stores {
        let dir = tempfile::tempdir().unwrap();
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
                TestClock::at(t0()),
            )
            .await
            .unwrap(),
        );
        let db = Arc::new(ResearchDb::open(&dir.path().join("shodh.db"), None).unwrap());
        Stores {
            snippets: SnippetService::new(store.clone(), db, "test"),
            store,
            dir,
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
        // A revision: the history moves with the snippet.
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
        let settled = settle_scopes(&s.store, &scopes, &workspaces, true)
            .await
            .unwrap();
        // Both versions of the source-filed snippet, and the memory: orphaned; the
        // user's snippet in the live workspace: Library-made.
        assert_eq!(
            settled,
            SettledScopes {
                library: 1,
                orphaned: 3
            }
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
        let again = settle_scopes(
            &s.store,
            &s.store.scopes().await.unwrap(),
            &workspaces,
            true,
        )
        .await
        .unwrap();
        assert_eq!(again.moved(), 0);
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
        settle_scopes(
            &s.store,
            &s.store.scopes().await.unwrap(),
            &workspaces,
            false,
        )
        .await
        .unwrap();
        assert_eq!(scope_of(&s.store, &kept.statement_id).await, live);
    }
}
