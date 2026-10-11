//! Slicing for constrained extraction, and version diffs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{core, with_research};
use shodh_ontology::{
    ClassField, Compatibility, Ontology, OntologyBuilder, OntologyDiff, PropertyField, SliceRole,
};

#[test]
fn invoice_text_slices_to_invoice_classes() {
    let ontology = core();
    let text = "TAX INVOICE\nInvoice No: INV-1043  Date: 30/09/2026\nAcme Industries Pvt Ltd, GSTIN 29ABCPE1234F1Z5\nGrand Total: ₹12,500.00";
    let slice = ontology.slice_for(text);
    let matched = slice.matched_classes();
    for class in ["Invoice", "Organization", "TaxId", "Amount"] {
        assert!(matched.contains(&class), "{class} not matched: {matched:?}");
    }
    assert!(!matched.contains(&"Episode"));
    assert!(!matched.contains(&"Contract"));
    let taxid = slice.matches.iter().find(|m| m.class == "TaxId").unwrap();
    assert_eq!(taxid.evidence.to_lowercase(), "gstin");

    let role = |id: &str| {
        slice
            .classes
            .iter()
            .find(|c| c.class.id == id)
            .map(|c| c.role)
    };
    assert_eq!(role("Document"), Some(SliceRole::Ancestor));
    assert_eq!(role("Thing"), Some(SliceRole::Ancestor));
    assert_eq!(role("LineItem"), Some(SliceRole::Referenced));
    assert_eq!(role("Note"), None);

    let props: Vec<&str> = slice.properties.iter().map(|p| p.id.as_str()).collect();
    for p in ["invoiceNumber", "issuedBy", "totalAmount", "gstin", "name"] {
        assert!(props.contains(&p), "{p} missing");
    }
    assert!(!props.contains(&"livesIn"));

    let prompt = slice.render_prompt();
    assert!(prompt.contains("Invoice (is-a Document):"), "{prompt}");
    assert!(
        prompt.contains("  - invoiceNumber: String [one, required]"),
        "{prompt}"
    );
    assert!(
        prompt.contains("  - issuedBy: -> Organization [one, required]"),
        "{prompt}"
    );
    assert!(prompt.contains("  - gstin: String /"), "{prompt}");
    assert!(
        prompt.contains("  - dueOn: Date [one, changes over time]"),
        "{prompt}"
    );
    assert!(prompt.contains("Referenced classes:"), "{prompt}");
    assert!(!prompt.contains("Episode"));
    assert_eq!(prompt, ontology.slice_for(text).render_prompt());
}

#[test]
fn gstin_value_alone_selects_tax_id_by_pattern() {
    let ontology = core();
    let slice = ontology.slice_for("Ref 07AAACR5055K1ZK dated 2026-01-01");
    let m = slice.matches.iter().find(|m| m.class == "TaxId").unwrap();
    assert_eq!(m.via, "pattern:gstin");
    assert_eq!(m.evidence, "07AAACR5055K1ZK");
}

#[test]
fn cue_terms_respect_word_boundaries() {
    let ontology = core();
    // "pan" inside "company" / "Japan" must not select TaxId.
    let slice = ontology.slice_for("Japanese companies expand");
    assert!(!slice.contains("TaxId"), "{:?}", slice.matches);
    // Neutral text selects nothing.
    assert!(ontology
        .slice_for("Blue skies over quiet hills.")
        .is_empty());
}

#[test]
fn research_text_slices_to_research_classes() {
    let ontology = with_research();
    let slice = ontology.slice_for(
        "Table 2: our method achieves 91.2% exact match on the SQuAD dev set, outperforming the baseline.",
    );
    let matched = slice.matched_classes();
    for class in ["Method", "Metric", "Result", "Snippet", "Dataset"] {
        assert!(matched.contains(&class), "{class} not matched: {matched:?}");
    }
    let prompt = slice.render_prompt();
    assert!(
        prompt.contains("  - resultValue: Decimal [one, required]"),
        "{prompt}"
    );
}

fn source(version: &str, body: &str) -> Ontology {
    OntologyBuilder::new()
        .add_source(
            "v.toml",
            format!("[core]\nid = \"t\"\nversion = \"{version}\"\nprefix = \"t\"\nnamespace = \"urn:t#\"\n\n[[class]]\nid = \"Thing\"\ndescription = \"root\"\n{body}"),
        )
        .build()
        .unwrap_or_else(|e| panic!("{e}"))
}

const BASE: &str = r#"
[[class]]
id = "Doc"
description = "A document."

[[class]]
id = "Memo"
parent = "Doc"
description = "A memo."

[[class]]
id = "Person"
description = "A person."

[[property]]
id = "author"
description = "Author."
domain = "Doc"
range = "Person"
cardinality = "one"

[[property]]
id = "nickname"
description = "Nickname."
domain = "Person"
range = "String"
cardinality = "one"
"#;

#[test]
fn identical_versions() {
    let diff = OntologyDiff::between(&source("1.0.0", BASE), &source("1.0.0", BASE));
    assert_eq!(diff.compatibility, Compatibility::Identical);
    assert!(diff.affected_classes.is_empty());
    assert!(diff.version_bump_sufficient());
    let core = core();
    assert_eq!(
        OntologyDiff::between(&core, &core).compatibility,
        Compatibility::Identical
    );
}

#[test]
fn cosmetic_change() {
    let changed = BASE.replace("description = \"A person.\"", "description = \"A human.\"");
    let diff = OntologyDiff::between(&source("1.0.0", BASE), &source("1.0.1", &changed));
    assert_eq!(diff.compatibility, Compatibility::Cosmetic);
    assert_eq!(diff.changed_classes[0].id, "Person");
    assert_eq!(
        diff.changed_classes[0].fields,
        vec![ClassField::Description]
    );
    assert!(diff.affected_classes.is_empty());
    assert!(diff.version_bump_sufficient());
}

#[test]
fn additive_change_marks_domain_and_subclasses() {
    let added = format!(
        "{BASE}\n[[property]]\nid = \"subject\"\ndescription = \"Subject line.\"\ndomain = \"Doc\"\nrange = \"String\"\ncardinality = \"one\"\n"
    );
    let old = source("1.0.0", BASE);
    let diff = OntologyDiff::between(&old, &source("1.1.0", &added));
    assert_eq!(diff.compatibility, Compatibility::Additive);
    assert_eq!(diff.added_properties, vec!["subject".to_owned()]);
    assert_eq!(
        diff.affected_classes,
        vec!["Doc".to_owned(), "Memo".to_owned()]
    );
    assert!(diff.version_bump_sufficient());
    // Same content under a patch bump is not enough.
    assert!(!OntologyDiff::between(&old, &source("1.0.1", &added)).version_bump_sufficient());
}

#[test]
fn breaking_changes() {
    let old = source("1.4.0", BASE);
    // Cardinality change.
    let many = BASE.replace(
        "range = \"Person\"\ncardinality = \"one\"",
        "range = \"Person\"\ncardinality = \"many\"",
    );
    let diff = OntologyDiff::between(&old, &source("1.5.0", &many));
    assert_eq!(diff.compatibility, Compatibility::Breaking);
    assert_eq!(diff.changed_properties[0].id, "author");
    assert_eq!(
        diff.changed_properties[0].fields,
        vec![PropertyField::Cardinality]
    );
    assert_eq!(
        diff.affected_classes,
        vec!["Doc".to_owned(), "Memo".to_owned()]
    );
    assert!(!diff.version_bump_sufficient());
    assert!(OntologyDiff::between(&old, &source("2.0.0", &many)).version_bump_sufficient());

    // Removal.
    let removed = BASE.replace(
        "\n[[property]]\nid = \"nickname\"\ndescription = \"Nickname.\"\ndomain = \"Person\"\nrange = \"String\"\ncardinality = \"one\"\n",
        "",
    );
    let diff = OntologyDiff::between(&old, &source("2.0.0", &removed));
    assert_eq!(diff.removed_properties, vec!["nickname".to_owned()]);
    assert_eq!(diff.compatibility, Compatibility::Breaking);
    assert_eq!(diff.affected_classes, vec!["Person".to_owned()]);

    // New required property on an existing class.
    let required = format!(
        "{BASE}\n[[property]]\nid = \"memoNumber\"\ndescription = \"No.\"\ndomain = \"Memo\"\nrange = \"String\"\ncardinality = \"one\"\nrequired = true\n"
    );
    assert_eq!(
        OntologyDiff::between(&old, &source("2.0.0", &required)).compatibility,
        Compatibility::Breaking
    );

    // 0.x: a minor bump is enough for breaking changes.
    let diff = OntologyDiff::between(&source("0.3.0", BASE), &source("0.4.0", &many));
    assert!(diff.version_bump_sufficient());
}

#[test]
fn adding_a_pack_is_additive() {
    let diff = OntologyDiff::between(&core(), &with_research());
    assert_eq!(diff.compatibility, Compatibility::Additive);
    assert!(diff.removed_classes.is_empty());
    assert!(diff.added_classes.contains(&"Paper".to_owned()));
    assert!(diff.affected_classes.contains(&"Paper".to_owned()));
}

fn with_pack(version: &str, cardinality: &str) -> Ontology {
    OntologyBuilder::new()
        .with_core()
        .add_source(
            "pack.toml",
            format!(
                "[pack]\nid = \"acme.pack\"\nversion = \"{version}\"\nprefix = \"acme\"\nnamespace = \"urn:acme#\"\n\n[[class]]\nid = \"Gizmo\"\ndescription = \"g\"\n\n[[property]]\nid = \"gizmoPart\"\ndescription = \"p\"\ndomain = \"Gizmo\"\nrange = \"String\"\ncardinality = \"{cardinality}\"\n"
            ),
        )
        .build()
        .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn version_checks_are_per_source() {
    // Adding a pack leaves the core untouched and is sufficient on its own.
    let diff = OntologyDiff::between(&core(), &with_research());
    assert!(diff.version_bump_sufficient());
    let research = diff
        .sources
        .iter()
        .find(|s| s.source == "shodh.research")
        .unwrap();
    assert_eq!(research.from, None);
    assert_eq!(
        diff.sources
            .iter()
            .find(|s| s.source == "shodh.core")
            .unwrap()
            .compatibility,
        Compatibility::Identical
    );

    // A breaking change inside a pack needs a major bump of that pack, not of the core.
    let old = with_pack("1.0.0", "one");
    let diff = OntologyDiff::between(&old, &with_pack("1.1.0", "many"));
    assert_eq!(diff.compatibility, Compatibility::Breaking);
    assert!(!diff.version_bump_sufficient());
    let diff = OntologyDiff::between(&old, &with_pack("2.0.0", "many"));
    assert!(diff.version_bump_sufficient());
    assert_eq!(diff.affected_classes, vec!["Gizmo".to_owned()]);

    // Removing a pack is breaking and cannot be satisfied by any version.
    let diff = OntologyDiff::between(&old, &core());
    assert_eq!(diff.compatibility, Compatibility::Breaking);
    assert!(!diff.version_bump_sufficient());
}

#[test]
fn statements_record_the_version_of_the_defining_source() {
    let ontology = with_pack("2.3.0", "one");
    let mut provenance = common::provenance("2026-10-03T09:00:00Z");
    provenance.page = None;
    let mut statement = shodh_ontology::Statement {
        id: "g1".to_owned(),
        class: "Gizmo".to_owned(),
        subject: None,
        properties: Default::default(),
        ontology_version: semver::Version::new(2, 1, 0),
        valid_from: None,
        provenance: Some(provenance),
    };
    assert!(ontology.validate(&statement).is_ok());
    statement.ontology_version = semver::Version::new(1, 0, 0);
    match ontology.validate(&statement) {
        Err(violations) => assert!(matches!(
            violations.as_slice(),
            [shodh_ontology::Violation::IncompatibleVersion { defined_by, .. }] if defined_by == "acme.pack"
        )),
        Ok(_) => panic!("expected an incompatible version"),
    }
}
