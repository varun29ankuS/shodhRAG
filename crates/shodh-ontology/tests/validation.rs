//! Statement validation: provenance, classes, domains, datatypes, patterns, cardinality.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{core, statement, valid, with_research};
use shodh_ontology::{EntityRef, Ontology, RawValue, Statement, Value, Violation};

fn invoice(extra: Vec<(&str, RawValue)>) -> Statement {
    let mut properties = vec![
        ("invoiceNumber", RawValue::text("INV-1043")),
        (
            "issuedBy",
            RawValue::Entity(EntityRef::typed("org-acme", "Organization")),
        ),
    ];
    properties.extend(extra);
    statement("inv-1043", "Invoice", properties)
}

fn violations(ontology: &Ontology, statement: &Statement) -> Vec<Violation> {
    match ontology.validate(statement) {
        Ok(v) => panic!("expected violations, got {v:?}"),
        Err(violations) => violations,
    }
}

fn single(ontology: &Ontology, statement: &Statement) -> Violation {
    let mut all = violations(ontology, statement);
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

fn codes(ontology: &Ontology, statement: &Statement) -> Vec<&'static str> {
    violations(ontology, statement)
        .iter()
        .map(Violation::code)
        .collect()
}

#[test]
fn valid_invoice_is_typed() {
    let ontology = core();
    let statement = invoice(vec![
        ("invoiceDate", RawValue::text("2026-09-30")),
        ("totalAmount", RawValue::money("0012500.50", "INR")),
        ("taxAmount", RawValue::text("1906.85 INR")),
        (
            "billedTo",
            RawValue::Entity(EntityRef::typed("p-1", "Person")),
        ),
        (
            "alias",
            RawValue::List(vec!["Bill 1043".into(), "1043".into(), "1043".into()]),
        ),
    ]);
    let valid = valid(&ontology, &statement);
    assert_eq!(valid.class(), "Invoice");
    assert_eq!(valid.values("totalAmount")[0].to_string(), "12500.5 INR");
    assert_eq!(valid.values("taxAmount")[0].to_string(), "1906.85 INR");
    assert_eq!(valid.values("invoiceDate")[0].to_string(), "2026-09-30");
    // Duplicate values in a Many property are collapsed.
    assert_eq!(valid.values("alias").len(), 2);
    assert_eq!(valid.provenance().generation, 3);
}

#[test]
fn provenance_is_mandatory_and_checked() {
    let ontology = core();
    let mut statement = invoice(vec![]);
    statement.provenance = None;
    assert_eq!(single(&ontology, &statement), Violation::MissingProvenance);

    let mut statement = invoice(vec![]);
    if let Some(p) = statement.provenance.as_mut() {
        p.confidence = 1.5;
    }
    assert_eq!(single(&ontology, &statement).code(), "invalid_provenance");

    let mut statement = invoice(vec![]);
    if let Some(p) = statement.provenance.as_mut() {
        p.page = Some(0);
    }
    assert_eq!(single(&ontology, &statement).code(), "invalid_provenance");

    let mut statement = invoice(vec![]);
    if let Some(p) = statement.provenance.as_mut() {
        p.source = " ".to_owned();
    }
    assert_eq!(single(&ontology, &statement).code(), "invalid_provenance");
}

#[test]
fn provenance_is_required_after_deserialization() {
    // Extractors hand statements over as serialized data; absent provenance must still be
    // caught after deserialization.
    let ontology = core();
    let text = r#"
id = "s1"
class = "Note"
ontology_version = "1.0.0"
[properties]
noteText = "Call the CA about advance tax."
"#;
    let statement: Statement = toml::from_str(text).unwrap();
    assert_eq!(single(&ontology, &statement), Violation::MissingProvenance);
}

#[test]
fn version_compatibility() {
    let ontology = core();
    let mut statement = invoice(vec![]);
    statement.ontology_version = semver::Version::new(2, 0, 0);
    assert_eq!(single(&ontology, &statement).code(), "incompatible_version");
    statement.ontology_version = semver::Version::new(1, 1, 0);
    assert_eq!(single(&ontology, &statement).code(), "incompatible_version");
    statement.ontology_version = semver::Version::new(1, 0, 0);
    assert!(ontology.validate(&statement).is_ok());
}

#[test]
fn class_and_domain_checks() {
    let ontology = core();
    let unknown = statement("s", "Spaceship", vec![]);
    assert_eq!(
        single(&ontology, &unknown),
        Violation::UnknownClass {
            class: "Spaceship".to_owned()
        }
    );

    let statement = invoice(vec![
        ("colour", RawValue::text("red")),
        ("livesIn", RawValue::entity("place-1")),
    ]);
    let mut found = codes(&ontology, &statement);
    found.sort_unstable();
    assert_eq!(found, vec!["not_in_domain", "unknown_property"]);

    let mut statement = invoice(vec![]);
    statement.subject = Some(EntityRef::typed("e1", "Person"));
    assert_eq!(
        single(&ontology, &statement).code(),
        "subject_class_mismatch"
    );
    statement.subject = Some(EntityRef::typed("e1", "Document"));
    assert!(ontology.validate(&statement).is_ok());
}

#[test]
fn inherited_properties_apply_to_subclasses() {
    let ontology = with_research();
    let paper = statement(
        "paper-1",
        "Paper",
        vec![
            ("title", RawValue::text("Attention Is All You Need")),
            ("name", RawValue::text("Transformer paper")),
            ("arxivId", RawValue::text("1706.03762v7")),
            ("publicationYear", RawValue::Integer(2017)),
        ],
    );
    valid(&ontology, &paper);
}

#[test]
fn cardinality_and_required() {
    let ontology = core();
    let statement = invoice(vec![(
        "invoiceDate",
        RawValue::List(vec!["2026-09-30".into(), "2026-10-01".into()]),
    )]);
    assert_eq!(
        single(&ontology, &statement),
        Violation::CardinalityExceeded {
            property: "invoiceDate".to_owned(),
            count: 2
        }
    );
    // A one-element list is fine for a One property.
    let statement = invoice(vec![(
        "invoiceDate",
        RawValue::List(vec!["2026-09-30".into()]),
    )]);
    valid(&ontology, &statement);

    let missing = statement_without(&["issuedBy"]);
    assert_eq!(
        single(&ontology, &missing),
        Violation::MissingRequired {
            property: "issuedBy".to_owned()
        }
    );
    let mut empty_list = invoice(vec![]);
    empty_list
        .properties
        .insert("issuedBy".to_owned(), RawValue::List(vec![]));
    assert_eq!(single(&ontology, &empty_list).code(), "missing_required");
}

fn statement_without(remove: &[&str]) -> Statement {
    let mut statement = invoice(vec![]);
    for key in remove {
        statement.properties.remove(*key);
    }
    statement
}

fn check(
    ontology: &Ontology,
    class: &str,
    base: Vec<(&str, RawValue)>,
    property: &str,
    value: RawValue,
) -> Result<Value, Violation> {
    let mut properties = base;
    properties.push((property, value));
    let statement = statement("s", class, properties);
    match ontology.validate(&statement) {
        Ok(valid) => Ok(valid.values(property)[0].clone()),
        Err(mut violations) => {
            assert_eq!(violations.len(), 1, "{violations:?}");
            Err(violations.remove(0))
        }
    }
}

fn invoice_base() -> Vec<(&'static str, RawValue)> {
    vec![
        ("invoiceNumber", RawValue::text("1")),
        ("issuedBy", RawValue::entity("o")),
    ]
}

#[test]
fn datatypes_parse_strictly() {
    let o = core();
    let inv = |p: &str, v: RawValue| check(&o, "Invoice", invoice_base(), p, v);
    // Date: ISO 8601 calendar date only.
    assert!(inv("invoiceDate", "2024-02-29".into()).is_ok());
    for bad in [
        "2024-1-5",
        "2024-02-30",
        "30/09/2026",
        "2026-09-30T00:00:00Z",
    ] {
        assert_eq!(
            inv("invoiceDate", bad.into()).unwrap_err().code(),
            "invalid_value",
            "{bad}"
        );
    }
    // Money: decimal + ISO 4217 code; never a float, never separators.
    assert!(inv("totalAmount", RawValue::money("100", "USD")).is_ok());
    assert!(inv("totalAmount", RawValue::money("1,250.00", "INR")).is_err());
    assert!(inv("totalAmount", RawValue::money("10", "Rs")).is_err());
    assert!(inv("totalAmount", "₹1250".into()).is_err());
    assert_eq!(
        inv("totalAmount", RawValue::Float(12.5))
            .unwrap_err()
            .code(),
        "type_mismatch"
    );
    // Relation given as text.
    assert_eq!(
        inv("billedTo", "Acme Pvt Ltd".into()).unwrap_err().code(),
        "type_mismatch"
    );
    // Relation range class.
    assert_eq!(
        inv(
            "issuedBy",
            RawValue::Entity(EntityRef::typed("x", "Person"))
        )
        .map(|_| ())
        .unwrap_err(),
        Violation::RangeClassMismatch {
            property: "issuedBy".to_owned(),
            expected: "Organization".to_owned(),
            found: "Person".to_owned()
        }
    );

    let line = |p: &str, v: RawValue| check(&o, "LineItem", vec![], p, v);
    // Decimal.
    assert_eq!(
        line("quantity", "002.50".into()).unwrap(),
        Value::Decimal("2.5".parse().unwrap())
    );
    assert!(line("quantity", RawValue::Integer(3)).is_ok());
    assert!(line("quantity", RawValue::Float(0.25)).is_ok());
    assert!(line("quantity", "1,000".into()).is_err());
    assert!(line("quantity", "1e3".into()).is_err());
    // Regex-constrained String (range String + pattern).
    assert!(line("hsnCode", "998313".into()).is_ok());
    assert_eq!(
        line("hsnCode", "99A".into()).unwrap_err().code(),
        "pattern_mismatch"
    );

    let event = |p: &str, v: RawValue| check(&o, "Event", vec![], p, v);
    // DateTime requires an offset.
    assert!(event("startsAt", "2026-10-03T10:00:00+05:30".into()).is_ok());
    assert!(event("startsAt", "2026-10-03T10:00:00".into()).is_err());
    assert!(event("startsAt", "2026-10-03".into()).is_err());

    let task = |p: &str, v: RawValue| check(&o, "Task", vec![], p, v);
    // Enum is exact.
    assert_eq!(
        task("taskStatus", "done".into()).unwrap(),
        Value::Enum("done".to_owned())
    );
    assert_eq!(
        task("taskStatus", "Done".into()).unwrap_err().code(),
        "not_in_enum"
    );

    let org = |p: &str, v: RawValue| check(&o, "Organization", vec![], p, v);
    assert!(org("website", "https://acme.example/in".into()).is_ok());
    assert!(org("website", "acme.example".into()).is_err());
    assert!(org("email", "accounts@acme.example".into()).is_ok());
    assert!(org("email", "accounts at acme".into()).is_err());
    assert!(org("name", RawValue::Integer(5)).is_err());

    let research = with_research();
    let metric = |v: RawValue| check(&research, "Metric", vec![], "higherIsBetter", v);
    assert_eq!(metric(true.into()).unwrap(), Value::Boolean(true));
    assert_eq!(metric("false".into()).unwrap(), Value::Boolean(false));
    assert!(metric("yes".into()).is_err());
    let paper = |v: RawValue| check(&research, "Paper", vec![], "publicationYear", v);
    assert_eq!(paper("2017".into()).unwrap(), Value::Integer(2017));
    assert!(paper("2017a".into()).is_err());
}

#[test]
fn gstin_and_pan_patterns() {
    let o = core();
    let tax = |p: &str, v: &str| check(&o, "TaxId", vec![], p, v.into());
    assert!(tax("gstin", "29ABCPE1234F1Z5").is_ok());
    assert!(tax("gstin", "07AAACR5055K1ZK").is_ok());
    for bad in [
        "29ABCPE1234F1X5",  // 14th character must be Z
        "29abcpe1234f1z5",  // lower case
        "29ABCPE1234F1Z",   // too short
        "29ABCPE1234F0Z5",  // entity code cannot be 0
        "29ABCPE1234F1Z55", // too long
        "AB29CPE1234F1Z5",  // state code must be digits
    ] {
        assert_eq!(
            tax("gstin", bad).unwrap_err().code(),
            "pattern_mismatch",
            "{bad}"
        );
    }
    assert!(tax("pan", "ABCPE1234F").is_ok());
    for bad in [
        "ABCDE1234F",  // 4th character must be a valid holder type
        "ABCP1234F",   // too short
        "ABCPE12345",  // last must be a letter
        "abcpe1234f",  // lower case
        " ABCPE1234F", // whitespace is not trimmed
    ] {
        assert_eq!(
            tax("pan", bad).unwrap_err().code(),
            "pattern_mismatch",
            "{bad}"
        );
    }
}

#[test]
fn all_violations_are_reported_together() {
    let ontology = core();
    let mut statement = statement(
        "",
        "Invoice",
        vec![
            ("invoiceDate", RawValue::text("yesterday")),
            ("totalAmount", RawValue::money("abc", "INR")),
        ],
    );
    statement.provenance = None;
    let mut found = codes(&ontology, &statement);
    found.sort_unstable();
    assert_eq!(
        found,
        vec![
            "invalid_id",
            "invalid_value",
            "invalid_value",
            "missing_provenance",
            "missing_required",
            "missing_required"
        ]
    );
}
