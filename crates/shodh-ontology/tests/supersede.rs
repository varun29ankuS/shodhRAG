//! Supersede decisions: temporal functional properties, accumulation, conflicts, identity.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{at, core, provenance, statement, valid};
use shodh_ontology::{
    EntityRef, IndependenceReason, Ontology, PropertyChange, RawValue, Statement,
    SubjectMatch, SupersedeDecision, ValidStatement, Value,
};

fn person(id: &str, when: &str, properties: Vec<(&str, RawValue)>) -> Statement {
    let mut s = statement(id, "Person", properties);
    s.subject = Some(EntityRef::new("person-varun"));
    s.provenance = Some(provenance(when));
    s
}

fn place(id: &str) -> RawValue {
    RawValue::Entity(EntityRef::typed(id, "Place"))
}

fn v(ontology: &Ontology, s: &Statement) -> ValidStatement {
    valid(ontology, s)
}

#[test]
fn temporal_one_property_supersedes_with_history() {
    let o = core();
    let old = v(&o, &person("s1", "2025-01-10T00:00:00Z", vec![("livesIn", place("pune"))]));
    let new = v(&o, &person("s2", "2026-06-01T00:00:00Z", vec![("livesIn", place("bengaluru"))]));
    let decision = o.supersedes(&old, &new);
    assert_eq!(
        decision,
        SupersedeDecision::Merge {
            matched_by: SubjectMatch::Subject {
                id: "person-varun".to_owned()
            },
            changes: vec![PropertyChange::Superseded {
                property: "livesIn".to_owned(),
                previous: Value::Entity(EntityRef::typed("pune", "Place")),
                current: Value::Entity(EntityRef::typed("bengaluru", "Place")),
                valid_to: at("2026-06-01T00:00:00Z"),
            }],
        }
    );
    assert_eq!(decision.superseded().len(), 1);
    // Deterministic.
    assert_eq!(decision, o.supersedes(&old, &new));
}

#[test]
fn older_incoming_value_becomes_history() {
    let o = core();
    let current = v(&o, &person("s2", "2026-06-01T00:00:00Z", vec![("livesIn", place("bengaluru"))]));
    let late_arrival = v(&o, &person("s1", "2025-01-10T00:00:00Z", vec![("livesIn", place("pune"))]));
    assert_eq!(
        o.supersedes(&current, &late_arrival).changes(),
        &[PropertyChange::Historical {
            property: "livesIn".to_owned(),
            value: Value::Entity(EntityRef::typed("pune", "Place")),
            valid_to: at("2026-06-01T00:00:00Z"),
        }]
    );
}

#[test]
fn valid_from_takes_precedence_over_extraction_time() {
    let o = core();
    // Extracted later, but the fact itself is older.
    let mut late = person("s3", "2026-09-01T00:00:00Z", vec![("livesIn", place("delhi"))]);
    late.valid_from = Some(at("2020-01-01T00:00:00Z"));
    let current = v(&o, &person("s2", "2026-06-01T00:00:00Z", vec![("livesIn", place("bengaluru"))]));
    assert!(matches!(
        o.supersedes(&current, &v(&o, &late)).changes(),
        [PropertyChange::Historical { .. }]
    ));
}

#[test]
fn time_ties_resolve_to_incoming() {
    let o = core();
    let a = v(&o, &person("a", "2026-06-01T00:00:00Z", vec![("livesIn", place("pune"))]));
    let b = v(&o, &person("b", "2026-06-01T00:00:00Z", vec![("livesIn", place("goa"))]));
    assert!(matches!(
        o.supersedes(&a, &b).changes(),
        [PropertyChange::Superseded { current: Value::Entity(e), .. }] if e.id == "goa"
    ));
    assert!(matches!(
        o.supersedes(&b, &a).changes(),
        [PropertyChange::Superseded { current: Value::Entity(e), .. }] if e.id == "pune"
    ));
}

#[test]
fn many_property_accumulates() {
    let o = core();
    let concept = |id: &str| RawValue::Entity(EntityRef::typed(id, "Concept"));
    let old = v(&o, &person("s1", "2026-01-01T00:00:00Z", vec![("interestedIn", concept("rust"))]));
    let new = v(
        &o,
        &person(
            "s2",
            "2026-02-01T00:00:00Z",
            vec![("interestedIn", RawValue::List(vec![concept("rust"), concept("ontologies")]))],
        ),
    );
    assert_eq!(
        o.supersedes(&old, &new).changes(),
        &[PropertyChange::Added {
            property: "interestedIn".to_owned(),
            values: vec![Value::Entity(EntityRef::typed("ontologies", "Concept"))],
        }]
    );
    let same = v(&o, &person("s3", "2026-03-01T00:00:00Z", vec![("interestedIn", concept("rust"))]));
    assert_eq!(
        o.supersedes(&old, &same).changes(),
        &[PropertyChange::Unchanged {
            property: "interestedIn".to_owned()
        }]
    );
}

#[test]
fn non_temporal_one_property_conflicts_instead_of_superseding() {
    let o = core();
    let invoice = |id: &str, total: &str, when: &str| {
        let mut s = statement(
            id,
            "Invoice",
            vec![
                ("invoiceNumber", RawValue::text("INV-1043")),
                ("issuedBy", RawValue::entity("org-acme")),
                ("totalAmount", RawValue::money(total, "INR")),
            ],
        );
        s.provenance = Some(provenance(when));
        v(&o, &s)
    };
    let a = invoice("a", "12500.00", "2026-01-01T00:00:00Z");
    let b = invoice("b", "12000.00", "2026-02-01T00:00:00Z");
    let decision = o.supersedes(&a, &b);
    let SupersedeDecision::Merge { matched_by, .. } = &decision else {
        panic!("expected merge: {decision:?}");
    };
    assert_eq!(
        matched_by,
        &SubjectMatch::IdentityKey {
            properties: vec!["invoiceNumber".to_owned(), "issuedBy".to_owned()]
        }
    );
    assert_eq!(decision.conflicts().len(), 1);
    assert!(decision.superseded().is_empty());
    // Equal money in a different lexical form is not a conflict.
    let c = invoice("c", "12500", "2026-03-01T00:00:00Z");
    assert!(o.supersedes(&a, &c).conflicts().is_empty());
}

#[test]
fn identity_key_without_subject_supersedes_preference() {
    let o = core();
    let preference = |id: &str, value: &str, when: &str| {
        let mut s = statement(
            id,
            "Preference",
            vec![
                ("preferenceHolder", RawValue::entity("person-varun")),
                ("preferenceTopic", RawValue::text("coffee")),
                ("preferenceValue", RawValue::text(value)),
            ],
        );
        s.provenance = Some(provenance(when));
        v(&o, &s)
    };
    let old = preference("p1", "black, no sugar", "2025-05-01T00:00:00Z");
    let new = preference("p2", "filter coffee with milk", "2026-05-01T00:00:00Z");
    let decision = o.supersedes(&old, &new);
    assert_eq!(decision.superseded().len(), 1);
    assert!(decision
        .changes()
        .iter()
        .filter(|c| !matches!(c, PropertyChange::Superseded { .. }))
        .all(|c| matches!(c, PropertyChange::Unchanged { .. })));

    let mut other_topic = statement(
        "p3",
        "Preference",
        vec![
            ("preferenceHolder", RawValue::entity("person-varun")),
            ("preferenceTopic", RawValue::text("tea")),
            ("preferenceValue", RawValue::text("masala chai")),
        ],
    );
    other_topic.provenance = Some(provenance("2026-05-02T00:00:00Z"));
    assert_eq!(
        o.supersedes(&old, &v(&o, &other_topic)),
        SupersedeDecision::Independent {
            reason: IndependenceReason::DifferentSubjects
        }
    );
}

#[test]
fn independence_reasons() {
    let o = core();
    let a = v(&o, &person("a", "2026-01-01T00:00:00Z", vec![("livesIn", place("pune"))]));
    let mut other = person("b", "2026-02-01T00:00:00Z", vec![("livesIn", place("goa"))]);
    other.subject = Some(EntityRef::new("person-someone-else"));
    assert_eq!(
        o.supersedes(&a, &v(&o, &other)),
        SupersedeDecision::Independent {
            reason: IndependenceReason::DifferentSubjects
        }
    );

    let mut anonymous = person("c", "2026-02-01T00:00:00Z", vec![("livesIn", place("goa"))]);
    anonymous.subject = None;
    assert_eq!(
        o.supersedes(&a, &v(&o, &anonymous)),
        SupersedeDecision::Independent {
            reason: IndependenceReason::UnknownIdentity
        }
    );

    let mut note = statement("n", "Note", vec![("noteText", RawValue::text("x"))]);
    note.subject = Some(EntityRef::new("person-varun"));
    assert_eq!(
        o.supersedes(&a, &v(&o, &note)),
        SupersedeDecision::Independent {
            reason: IndependenceReason::UnrelatedClasses
        }
    );
}

#[test]
fn subclass_statements_merge_with_superclass_statements() {
    let o = core();
    let party = |class: &str, id: &str, when: &str, gstin_ref: &str| {
        let mut s = statement(id, class, vec![("hasTaxId", RawValue::entity(gstin_ref))]);
        s.provenance = Some(provenance(when));
        v(&o, &s)
    };
    let a = party("Party", "a", "2026-01-01T00:00:00Z", "tax-29ABCPE1234F1Z5");
    let b = party("Organization", "b", "2026-02-01T00:00:00Z", "tax-29ABCPE1234F1Z5");
    assert!(matches!(
        o.supersedes(&a, &b),
        SupersedeDecision::Merge {
            matched_by: SubjectMatch::IdentityKey { .. },
            ..
        }
    ));
}
