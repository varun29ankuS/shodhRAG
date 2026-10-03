//! Statement store behaviour: put, supersede, history, as-of, search, scope, forget, expiry.

use chrono::Duration;
use shodh_ontology::{EntityRef, PropertyChange, RawValue};

use super::testing::{fixture, note, preference, statement, t0};
use super::{PropertyFilter, PutIntent, PutOutcome, Scope, StatementError, StatementQuery};

fn current(classes: &[&str]) -> StatementQuery {
    StatementQuery {
        classes: classes.iter().map(|c| c.to_string()).collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn put_validates_and_stores_with_dynamics() {
    let f = fixture().await;
    let out = f
        .store
        .put(
            note("n1", "The wifi password is on the fridge", t0()),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    assert_eq!(out, PutOutcome::Added { id: "n1".into() });
    let stored = f.store.get("n1").await.unwrap();
    assert_eq!(stored.text, "The wifi password is on the fridge");
    assert_eq!(stored.ontology_source, "shodh.core");
    assert_eq!(stored.valid_from, t0());
    assert!(stored.is_current_at(t0()));
    let state = f.store.dynamics().get("n1").unwrap().unwrap();
    assert_eq!(state.strength, 1.0);
    assert!(state.importance > 0.0);

    // Invalid statements are rejected with every violation; nothing is stored.
    let mut bad = note("n2", "x", t0());
    bad.properties
        .insert("dueOn".into(), RawValue::text("tomorrow"));
    bad.provenance = None;
    match f.store.put(bad, Scope::Global, PutIntent::Auto).await {
        Err(StatementError::Invalid(violations)) => assert!(violations.len() >= 2),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        f.store.get("n2").await,
        Err(StatementError::NotFound(_))
    ));
    // Ids are never reused.
    assert!(matches!(
        f.store
            .put(
                note("n1", "other text", t0()),
                Scope::Global,
                PutIntent::Auto
            )
            .await,
        Err(StatementError::DuplicateId(_))
    ));
}

#[tokio::test]
async fn temporal_values_supersede_with_history_and_as_of() {
    let f = fixture().await;
    let t1 = t0();
    let t2 = t0() + Duration::days(30);
    f.store
        .put(
            preference("p1", "coffee", "black", t1),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    f.clock.set(t2);
    let out = f
        .store
        .put(
            preference("p2", "coffee", "with oat milk", t2),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        PutOutcome::Updated {
            id: "p2".into(),
            superseded: vec!["p1".into()]
        }
    );
    let old = f.store.get("p1").await.unwrap();
    assert_eq!(old.valid_to, Some(t2));
    assert_eq!(old.superseded_by.as_deref(), Some("p2"));

    // Now: only the new value is current.
    let now = f.store.query(&current(&["Preference"])).await.unwrap();
    assert_eq!(now.iter().map(|s| s.id()).collect::<Vec<_>>(), vec!["p2"]);
    // As of before the change: the old value.
    let before = f
        .store
        .query(&StatementQuery {
            as_of: Some(t1 + Duration::days(1)),
            ..current(&["Preference"])
        })
        .await
        .unwrap();
    assert_eq!(
        before.iter().map(|s| s.id()).collect::<Vec<_>>(),
        vec!["p1"]
    );
    // History keeps both, oldest first, closed at the change.
    let history = f
        .store
        .history("p2", Some("preferenceValue"))
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].values, vec!["preferenceValue = black"]);
    assert_eq!(history[0].valid_to, Some(t2));
    assert_eq!(history[1].values, vec!["preferenceValue = with oat milk"]);
    assert_eq!(history[1].valid_to, None);
    // Including history returns both, newest first.
    let all = f
        .store
        .query(&StatementQuery {
            include_history: true,
            ..current(&["Preference"])
        })
        .await
        .unwrap();
    assert_eq!(
        all.iter().map(|s| s.id()).collect::<Vec<_>>(),
        vec!["p2", "p1"]
    );
    // Search sees only the current value by default; the closed row was updated in place
    // after the index was built and is still excluded by the prefilter.
    let hits = f
        .store
        .search("coffee", &StatementQuery::default(), 5)
        .await
        .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.stored.id()).collect::<Vec<_>>(),
        vec!["p2"]
    );
    assert!(
        hits[0].lexical,
        "full-text search finds rows appended after indexing"
    );
    let past = f
        .store
        .search(
            "coffee",
            &StatementQuery {
                as_of: Some(t1 + Duration::days(1)),
                ..Default::default()
            },
            5,
        )
        .await
        .unwrap();
    assert_eq!(
        past.iter().map(|h| h.stored.id()).collect::<Vec<_>>(),
        vec!["p1"]
    );
}

#[tokio::test]
async fn equal_facts_reinforce_older_facts_become_history_and_conflicts_are_reported() {
    let f = fixture().await;
    f.store
        .put(
            preference("p1", "tea", "green", t0()),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    // Same fact again, later: NOOP + reinforcement (anchor moves).
    f.clock.advance_days(100);
    let out = f
        .store
        .put(
            preference("p1b", "tea", "green", f.clock_now()),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        PutOutcome::Unchanged {
            existing: "p1".into()
        }
    );
    assert!(matches!(
        f.store.get("p1b").await,
        Err(StatementError::NotFound(_))
    ));
    let state = f.store.dynamics().get("p1").unwrap().unwrap();
    assert_eq!(state.anchor_at, f.clock_now());

    // An older value than the current one is kept as history only.
    let out = f
        .store
        .put(
            preference("p0", "tea", "black", t0() - Duration::days(365)),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        PutOutcome::Historical {
            id: "p0".into(),
            current: "p1".into()
        }
    );
    let p0 = f.store.get("p0").await.unwrap();
    assert_eq!(p0.valid_to, Some(t0()));
    assert!(f.store.get("p1").await.unwrap().valid_to.is_none());

    // A functional, non-temporal value never changes silently.
    let mut a = statement(
        "person-a",
        "Person",
        &[("name", RawValue::text("Asha Rao"))],
        t0(),
    );
    a.subject = Some(EntityRef::typed("person:asha", "Person"));
    f.store
        .put(a, Scope::Global, PutIntent::Auto)
        .await
        .unwrap();
    let mut b = statement(
        "person-b",
        "Person",
        &[("name", RawValue::text("Asha R."))],
        f.clock_now(),
    );
    b.subject = Some(EntityRef::typed("person:asha", "Person"));
    match f
        .store
        .put(b, Scope::Global, PutIntent::Auto)
        .await
        .unwrap()
    {
        PutOutcome::Conflict {
            existing,
            conflicts,
        } => {
            assert_eq!(existing, "person-a");
            assert!(
                matches!(&conflicts[0], PropertyChange::Conflict { property, .. } if property == "name")
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        f.store.get("person-b").await,
        Err(StatementError::NotFound(_))
    ));

    // Many-valued properties accumulate into the current statement.
    let mut c = statement(
        "person-c",
        "Person",
        &[("alias", RawValue::text("AR"))],
        f.clock_now(),
    );
    c.subject = Some(EntityRef::typed("person:asha", "Person"));
    let out = f
        .store
        .put(c, Scope::Global, PutIntent::Auto)
        .await
        .unwrap();
    assert_eq!(
        out,
        PutOutcome::Updated {
            id: "person-c".into(),
            superseded: vec!["person-a".into()]
        }
    );
    let merged = f.store.get("person-c").await.unwrap();
    assert!(
        merged.text.contains("Asha Rao") && merged.text.contains("AR"),
        "{}",
        merged.text
    );
}

#[tokio::test]
async fn explicit_supersede_edits_notes_and_keeps_the_chain() {
    let f = fixture().await;
    f.store
        .put(
            note("n1", "Dentist on Friday", t0()),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    // The same note text is a no-op.
    assert_eq!(
        f.store
            .put(
                note("n1-dup", "Dentist on Friday", t0()),
                Scope::Global,
                PutIntent::Auto
            )
            .await
            .unwrap(),
        PutOutcome::Unchanged {
            existing: "n1".into()
        }
    );
    f.clock.advance_days(1);
    let out = f
        .store
        .put(
            note("n2", "Dentist moved to Monday", f.clock_now()),
            Scope::Global,
            PutIntent::Supersede {
                target: "n1".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        PutOutcome::Updated {
            id: "n2".into(),
            superseded: vec!["n1".into()]
        }
    );
    let history = f.store.history("n2", None).await.unwrap();
    assert_eq!(
        history
            .iter()
            .map(|h| h.statement_id.as_str())
            .collect::<Vec<_>>(),
        vec!["n1", "n2"]
    );
    assert_eq!(f.store.history_ids("n1").await.unwrap(), vec!["n1", "n2"]);
    // A closed statement cannot be superseded again, and scopes must match.
    assert!(matches!(
        f.store
            .put(
                note("n3", "x", f.clock_now()),
                Scope::Global,
                PutIntent::Supersede {
                    target: "n1".into()
                }
            )
            .await,
        Err(StatementError::InvalidSupersede { .. })
    ));
    assert!(matches!(
        f.store
            .put(
                note("n4", "y", f.clock_now()),
                Scope::Workspace("w".into()),
                PutIntent::Supersede {
                    target: "n2".into()
                }
            )
            .await,
        Err(StatementError::InvalidSupersede { .. })
    ));
    // Unrelated classes cannot supersede each other.
    assert!(matches!(
        f.store
            .put(
                preference("p", "x", "y", f.clock_now()),
                Scope::Global,
                PutIntent::Supersede {
                    target: "n2".into()
                }
            )
            .await,
        Err(StatementError::InvalidSupersede { .. })
    ));
}

#[tokio::test]
async fn scopes_are_isolated() {
    let f = fixture().await;
    let ws_a = Scope::Workspace("a".into());
    let ws_b = Scope::Workspace("b".into());
    f.store
        .put(
            preference("pa", "editor", "vim", t0()),
            ws_a.clone(),
            PutIntent::Auto,
        )
        .await
        .unwrap();
    // The same identity in another scope is independent, not a supersede.
    let out = f
        .store
        .put(
            preference("pb", "editor", "emacs", t0()),
            ws_b.clone(),
            PutIntent::Auto,
        )
        .await
        .unwrap();
    assert_eq!(out, PutOutcome::Added { id: "pb".into() });
    f.store
        .put(
            note("g", "Global editor note", t0()),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();

    let from_b = f
        .store
        .search(
            "editor",
            &StatementQuery {
                scopes: ws_b.visible(),
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    let ids: Vec<&str> = from_b.iter().map(|h| h.stored.id()).collect();
    assert!(ids.contains(&"pb") && ids.contains(&"g"));
    assert!(!ids.contains(&"pa"), "{ids:?}");
    let global_only = f
        .store
        .query(&StatementQuery {
            scopes: Scope::Global.visible(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        global_only.iter().map(|s| s.id()).collect::<Vec<_>>(),
        vec!["g"]
    );
}

#[tokio::test]
async fn property_and_subject_queries() {
    let f = fixture().await;
    for (id, topic, value) in [("p1", "coffee", "black"), ("p2", "music", "jazz")] {
        f.store
            .put(
                preference(id, topic, value, t0()),
                Scope::Global,
                PutIntent::Auto,
            )
            .await
            .unwrap();
    }
    let jazz = f
        .store
        .query(&StatementQuery {
            properties: vec![PropertyFilter {
                property: "preferenceValue".into(),
                equals: "jazz".into(),
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(jazz.iter().map(|s| s.id()).collect::<Vec<_>>(), vec!["p2"]);
    let mut asha = statement("s1", "Person", &[("name", RawValue::text("Asha"))], t0());
    asha.subject = Some(EntityRef::typed("person:asha", "Person"));
    f.store
        .put(asha, Scope::Global, PutIntent::Auto)
        .await
        .unwrap();
    let about = f
        .store
        .query(&StatementQuery {
            subject: Some("person:asha".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(about.iter().map(|s| s.id()).collect::<Vec<_>>(), vec!["s1"]);
    // Classes include their subclasses.
    let parties = f.store.query(&current(&["Party"])).await.unwrap();
    assert_eq!(
        parties.iter().map(|s| s.id()).collect::<Vec<_>>(),
        vec!["s1"]
    );
}

#[tokio::test]
async fn forgotten_statements_disappear_but_are_kept() {
    let f = fixture().await;
    f.store
        .put(
            note("n1", "Locker code 4411", t0()),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    let before = f.store.forget("n1").await.unwrap();
    assert_eq!(before.id(), "n1");
    assert!(matches!(
        f.store.get("n1").await,
        Err(StatementError::NotFound(_))
    ));
    assert!(f
        .store
        .search("locker code", &StatementQuery::default(), 5)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        f.store.forget("n1").await,
        Err(StatementError::NotFound(_))
    ));
    // The id stays taken: the row is soft-deleted, not removed.
    assert!(matches!(
        f.store
            .put(note("n1", "again", t0()), Scope::Global, PutIntent::Auto)
            .await,
        Err(StatementError::DuplicateId(_))
    ));
}

#[tokio::test]
async fn expiry_ends_tasks_after_their_deadline_and_grace() {
    let f = fixture().await;
    let task = statement(
        "t1",
        "Task",
        &[
            ("name", RawValue::text("File the GST return")),
            ("dueOn", RawValue::text("2026-01-20")),
        ],
        t0(),
    );
    f.store
        .put(task, Scope::Global, PutIntent::Auto)
        .await
        .unwrap();
    let stored = f.store.get("t1").await.unwrap();
    // End of the due date plus the core Task grace of 30 days.
    let expected = chrono::NaiveDate::from_ymd_opt(2026, 2, 20)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    assert_eq!(stored.expires_at, Some(expected));
    let as_of = |at| StatementQuery {
        as_of: Some(at),
        ..current(&["Task"])
    };
    assert_eq!(
        f.store
            .query(&as_of(expected - Duration::seconds(1)))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(f.store.query(&as_of(expected)).await.unwrap().is_empty());
    // Still listed as history.
    let all = f
        .store
        .query(&StatementQuery {
            include_history: true,
            ..current(&["Task"])
        })
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
}

impl super::testing::Fixture {
    fn clock_now(&self) -> chrono::DateTime<chrono::Utc> {
        use super::Clock;
        self.clock.now()
    }
}
