//! Loader: built-in content, merge rules, file:line errors, dynamics.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{core, line_of, with_research};
use shodh_ontology::{
    builtin, Cardinality, Datatype, Expiry, Layer, LoadErrorKind, LoadErrors, OntologyBuilder,
    Range,
};

const EXT_HEADER: &str = "[extension]\nid = \"acme.logistics\"\nversion = \"0.1.0\"\nprefix = \"acme\"\nnamespace = \"urn:acme:logistics#\"\n";

fn ext(body: &str) -> String {
    format!("{EXT_HEADER}{body}")
}

fn errors_of(result: Result<shodh_ontology::Ontology, LoadErrors>) -> LoadErrors {
    match result {
        Ok(_) => panic!("expected load errors"),
        Err(errors) => errors,
    }
}

#[test]
fn core_defines_every_required_class_and_relation() {
    let ontology = core();
    assert_eq!(ontology.id(), "shodh.core");
    assert_eq!(ontology.version().to_string(), "1.0.0");
    for class in [
        "Thing", "Party", "Organization", "Person", "Document", "Invoice", "LineItem",
        "Contract", "Clause", "Obligation", "Payment", "Address", "Place", "TaxId", "Amount",
        "Period", "Event", "Task", "Project", "Preference", "Decision", "Episode", "Procedure",
        "Concept", "Note",
    ] {
        assert!(ontology.class(class).is_some(), "missing class {class}");
    }
    let expect = |id: &str, cardinality: Cardinality, temporal: bool| {
        let property = ontology
            .property(id)
            .unwrap_or_else(|| panic!("missing property {id}"));
        assert_eq!(property.cardinality, cardinality, "{id} cardinality");
        assert_eq!(property.temporal, temporal, "{id} temporal");
    };
    for relation in [
        "issuedBy", "billedTo", "effectiveOn", "expiresOn", "renewsOn", "amountOf", "paidBy",
    ] {
        expect(relation, Cardinality::One, relation.ends_with("On"));
    }
    for relation in [
        "partyTo", "signedBy", "references", "supersedes", "worksOn", "advisorOf",
        "interestedIn", "prefers",
    ] {
        expect(relation, Cardinality::Many, false);
    }
    expect("livesIn", Cardinality::One, true);
    let gstin = ontology.property("gstin").unwrap();
    assert!(gstin.pattern.is_some());
    assert_eq!(gstin.range, Range::Datatype(Datatype::String));
    assert!(ontology.property("pan").unwrap().pattern.is_some());
    assert_eq!(
        ontology.property("issuedBy").unwrap().range,
        Range::Class("Organization".to_owned())
    );
    assert!(ontology.is_subclass_of("Invoice", "Document"));
    assert!(ontology.is_subclass_of("Person", "Thing"));
}

#[test]
fn core_dynamics_follow_memory_design() {
    let ontology = core();
    let half_life = |c: &str| ontology.class(c).unwrap().dynamics.decay_half_life_days;
    let reinforcement = |c: &str| ontology.class(c).unwrap().dynamics.reinforcement;
    // Identity and preferences decay slowly, episodes fast.
    assert!(half_life("Person").unwrap() >= 3650.0);
    assert!(half_life("Preference").unwrap() >= 365.0);
    assert!(half_life("Episode").unwrap() <= 30.0);
    // Procedures are reinforced the most.
    let max = ontology
        .classes()
        .iter()
        .map(|c| c.dynamics.reinforcement)
        .fold(0.0, f64::max);
    assert_eq!(reinforcement("Procedure"), max);
    // Records do not decay.
    assert_eq!(half_life("Invoice"), None);
    // Deadlines expire.
    assert_eq!(
        ontology.class("Task").unwrap().dynamics.expires_after,
        Some(Expiry::AtProperty {
            property: "dueOn".to_owned(),
            grace_days: 30.0
        })
    );
    assert!(ontology
        .class("Obligation")
        .unwrap()
        .dynamics
        .expires_after
        .is_some());
}

#[test]
fn research_pack_loads_with_n_ary_result() {
    let ontology = with_research();
    let pack = ontology.packs().next().unwrap();
    assert_eq!(pack.id, "shodh.research");
    assert_eq!(pack.layer, Layer::Pack);
    for class in [
        "Paper", "Author", "Venue", "Method", "Dataset", "Metric", "Result", "Snippet",
    ] {
        let c = ontology.class(class).unwrap_or_else(|| panic!("missing {class}"));
        assert_eq!(c.source, "shodh.research");
    }
    assert!(ontology.is_subclass_of("Paper", "Document"));
    assert!(ontology.is_subclass_of("Author", "Person"));
    for relation in ["cites", "usesMethod", "evaluatedOn", "proposedIn", "authoredBy"] {
        assert!(ontology.property(relation).unwrap().is_relation());
    }
    let required: Vec<&str> = ontology
        .properties_of("Result")
        .into_iter()
        .filter(|p| p.required)
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(
        required,
        vec!["resultMethod", "resultDataset", "resultMetric", "resultValue"]
    );
    let snippet: Vec<&str> = ontology
        .properties_of("Snippet")
        .into_iter()
        .map(|p| p.id.as_str())
        .collect();
    for p in ["snippetImage", "snippetText", "snippetPage", "snippetRect", "snippetOf"] {
        assert!(snippet.contains(&p), "Snippet lacks {p}");
    }
}

#[test]
fn extension_adds_classes_and_properties_to_core_classes() {
    let ontology = OntologyBuilder::new()
        .with_core()
        .add_source(
            "workspace/logistics.toml",
            ext(r#"
[[class]]
id = "Shipment"
parent = "Document"
description = "A consignment of goods."
cue_terms = ["shipment", "consignment", "awb"]

[[property]]
id = "consignee"
description = "Receiver of the shipment."
domain = "Shipment"
range = "Party"
cardinality = "one"
required = true

[[property]]
id = "purchaseOrder"
description = "PO number the invoice refers to."
domain = "Invoice"
range = "String"
cardinality = "one"
"#),
        )
        .build()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(ontology.is_subclass_of("Shipment", "Document"));
    assert_eq!(ontology.class("Shipment").unwrap().source, "acme.logistics");
    let invoice_props: Vec<&str> = ontology
        .properties_of("Invoice")
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert!(invoice_props.contains(&"purchaseOrder"));
    // Inherited from Document and Thing.
    let shipment_props: Vec<&str> = ontology
        .properties_of("Shipment")
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert!(shipment_props.contains(&"title"));
    assert!(shipment_props.contains(&"name"));
    assert_eq!(ontology.sources().len(), 2);
}

#[test]
fn extension_cannot_redefine_core_class_and_error_names_both_locations() {
    let body = "\n[[class]]\nid = \"Invoice\"\ndescription = \"My own invoice.\"\n";
    let text = ext(body);
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("workspace/ext.toml", text.clone())
            .build(),
    );
    assert_eq!(errors.errors().len(), 1, "{errors}");
    let error = &errors.errors()[0];
    let location = error.location.as_ref().unwrap();
    assert_eq!(location.file, "workspace/ext.toml");
    assert_eq!(location.line, line_of(&text, "id = \"Invoice\""));
    assert_eq!(location.line, 8);
    match &error.kind {
        LoadErrorKind::Redefinition { id, first } => {
            assert_eq!(id, "Invoice");
            assert_eq!(first.file, builtin::CORE_NAME);
            assert_eq!(first.line, line_of(builtin::CORE_TOML, "id = \"Invoice\""));
        }
        other => panic!("unexpected error {other:?}"),
    }
    let rendered = error.to_string();
    assert!(
        rendered.starts_with("workspace/ext.toml:8:"),
        "rendered: {rendered}"
    );
    assert!(rendered.contains(&format!(
        "{}:{}",
        builtin::CORE_NAME,
        line_of(builtin::CORE_TOML, "id = \"Invoice\"")
    )));
}

#[test]
fn extension_cannot_redefine_core_property() {
    let text = ext("\n[[property]]\nid = \"invoiceNumber\"\ndescription = \"x\"\ndomain = \"Invoice\"\nrange = \"Integer\"\ncardinality = \"one\"\n");
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("workspace/ext.toml", text)
            .build(),
    );
    assert!(matches!(
        &errors.errors()[0].kind,
        LoadErrorKind::Redefinition { id, .. } if id == "invoiceNumber"
    ));
}

#[test]
fn extension_cannot_add_required_property_to_core_class() {
    let text = ext("\n[[property]]\nid = \"costCentre\"\ndescription = \"x\"\ndomain = \"Invoice\"\nrange = \"String\"\ncardinality = \"one\"\nrequired = true\n");
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("workspace/ext.toml", text)
            .build(),
    );
    let error = &errors.errors()[0];
    assert_eq!(error.location.as_ref().unwrap().line, 8);
    assert!(matches!(
        &error.kind,
        LoadErrorKind::RequiredOnForeignClass { owner, .. } if owner == "shodh.core"
    ));
}

#[test]
fn authoring_errors_carry_file_and_line() {
    let text = ext(r#"
[[class]]
id = "Crate"
parent = "Box"
description = "x"

[[class]]
id = "Date"
description = "clashes with a datatype"

[[property]]
id = "tags"
description = "x"
domain = "Crate"
range = "String"
cardinality = "many"
temporal = true

[[property]]
id = "weight"
description = "x"
domain = "Crate"
range = "Integer"
cardinality = "one"
pattern = '[0-9]+'

[[property]]
id = "colour"
description = "x"
domain = "Crate"
range = "Enum"
cardinality = "one"

[[property]]
id = "owner"
description = "x"
domain = "Crate"
range = "Nobody"
cardinality = "one"
"#);
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("ext.toml", text.clone())
            .build(),
    );
    let find = |pred: &dyn Fn(&LoadErrorKind) -> bool| {
        errors
            .errors()
            .iter()
            .find(|e| pred(&e.kind))
            .unwrap_or_else(|| panic!("missing error in {errors}"))
            .location
            .clone()
            .unwrap()
            .line
    };
    assert_eq!(
        find(&|k| matches!(k, LoadErrorKind::UnknownClass(c) if c == "Box")),
        line_of(&text, "parent = \"Box\"")
    );
    assert_eq!(
        find(&|k| matches!(k, LoadErrorKind::InvalidId { id, .. } if id == "Date")),
        line_of(&text, "id = \"Date\"")
    );
    assert_eq!(
        find(&|k| matches!(k, LoadErrorKind::InvalidProperty { property, reason } if property == "tags" && reason.contains("temporal"))),
        line_of(&text, "id = \"tags\"")
    );
    assert_eq!(
        find(&|k| matches!(k, LoadErrorKind::InvalidProperty { property, .. } if property == "weight")),
        line_of(&text, "id = \"weight\"")
    );
    assert_eq!(
        find(&|k| matches!(k, LoadErrorKind::InvalidProperty { property, .. } if property == "colour")),
        line_of(&text, "id = \"colour\"")
    );
    assert_eq!(
        find(&|k| matches!(k, LoadErrorKind::UnknownRange(r) if r == "Nobody")),
        line_of(&text, "id = \"owner\"")
    );
}

#[test]
fn toml_syntax_and_schema_errors_have_lines() {
    let text = ext("\n[[class]]\nid = \"Crate\"\ndescripton = \"typo\"\n");
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("ext.toml", text)
            .build(),
    );
    let error = &errors.errors()[0];
    assert!(matches!(error.kind, LoadErrorKind::Syntax(_)));
    let line = error.location.as_ref().unwrap().line;
    assert!((7..=9).contains(&line), "line {line}: {error}");

    let broken = ext("\n[[class]\nid = \"X\"\n");
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("broken.toml", broken)
            .build(),
    );
    assert_eq!(errors.errors()[0].location.as_ref().unwrap().line, 7);
}

#[test]
fn structural_rules() {
    // No core.
    let errors = errors_of(OntologyBuilder::new().add_source("e.toml", ext("")).build());
    assert!(matches!(errors.errors()[0].kind, LoadErrorKind::CoreCount(0)));

    // Unknown built-in pack.
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .with_builtin_pack("astrology")
            .build(),
    );
    assert!(matches!(
        &errors.errors()[0].kind,
        LoadErrorKind::UnknownBuiltinPack(name) if name == "astrology"
    ));

    // Pack requirement not met.
    let pack = "[pack]\nid = \"p\"\nversion = \"1.0.0\"\nprefix = \"p\"\nnamespace = \"urn:p#\"\nrequires = { \"shodh.core\" = \"^2\" }\n";
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("p.toml", pack)
            .build(),
    );
    assert!(matches!(
        &errors.errors()[0].kind,
        LoadErrorKind::UnmetRequirement { id, .. } if id == "shodh.core"
    ));

    // Inheritance cycle.
    let cycle = ext("\n[[class]]\nid = \"A\"\nparent = \"B\"\ndescription = \"a\"\n\n[[class]]\nid = \"B\"\nparent = \"A\"\ndescription = \"b\"\n");
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("c.toml", cycle)
            .build(),
    );
    assert!(errors
        .errors()
        .iter()
        .any(|e| matches!(e.kind, LoadErrorKind::InheritanceCycle(_))));

    // Duplicate namespace prefix.
    let clash = "[extension]\nid = \"x\"\nversion = \"1.0.0\"\nprefix = \"shodh\"\nnamespace = \"urn:x#\"\n";
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("x.toml", clash)
            .build(),
    );
    assert!(matches!(
        errors.errors()[0].kind,
        LoadErrorKind::InvalidNamespace(_)
    ));
}

#[test]
fn dynamics_are_inherited_and_validated() {
    let ontology = OntologyBuilder::new()
        .with_core()
        .add_source(
            "ext.toml",
            ext("\n[[class]]\nid = \"Chore\"\nparent = \"Task\"\ndescription = \"A household task.\"\n"),
        )
        .build()
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        ontology.class("Chore").unwrap().dynamics,
        ontology.class("Task").unwrap().dynamics
    );

    let bad = ext("\n[[class]]\nid = \"Fad\"\ndescription = \"x\"\n[class.dynamics]\ndecay_half_life_days = 0\nreinforcement = 1.5\n\n[[class]]\nid = \"Gig\"\ndescription = \"x\"\n[class.dynamics]\nexpires_after = { property = \"name\" }\n\n[[class]]\nid = \"Fling\"\ndescription = \"x\"\n[class.dynamics]\nexpires_after = { days = 3, property = \"name\" }\n");
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source("ext.toml", bad.clone())
            .build(),
    );
    for class in ["Fad", "Gig", "Fling"] {
        let error = errors
            .errors()
            .iter()
            .find(|e| matches!(&e.kind, LoadErrorKind::InvalidDynamics { class: c, .. } if c == class))
            .unwrap_or_else(|| panic!("no dynamics error for {class}: {errors}"));
        assert_eq!(
            error.location.as_ref().unwrap().line,
            line_of(&bad, &format!("id = \"{class}\""))
        );
    }
}

#[test]
fn add_dir_loads_extensions_in_name_order() {
    let dir = std::env::temp_dir().join(format!(
        "shodh-ontology-test-{}-{}",
        std::process::id(),
        line!()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("b.toml"),
        "[extension]\nid = \"b\"\nversion = \"1.0.0\"\nprefix = \"b\"\nnamespace = \"urn:b#\"\nrequires = { \"a\" = \"^1\" }\n\n[[class]]\nid = \"Bee\"\nparent = \"Ant\"\ndescription = \"b\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("a.toml"),
        "[extension]\nid = \"a\"\nversion = \"1.0.0\"\nprefix = \"a\"\nnamespace = \"urn:a#\"\n\n[[class]]\nid = \"Ant\"\ndescription = \"a\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("ignored.txt"), "not toml").unwrap();
    let result = OntologyBuilder::new().with_core().add_dir(&dir).build();
    std::fs::remove_dir_all(&dir).unwrap();
    let ontology = result.unwrap_or_else(|e| panic!("{e}"));
    assert!(ontology.is_subclass_of("Bee", "Ant"));
    let ids: Vec<&str> = ontology.packs().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b"]);
}

#[test]
fn forward_reference_to_later_layer_is_rejected() {
    // An extension referenced by a pack: packs load before extensions.
    let pack = "[pack]\nid = \"p\"\nversion = \"1.0.0\"\nprefix = \"p\"\nnamespace = \"urn:p#\"\n\n[[class]]\nid = \"Gadget\"\nparent = \"Widget\"\ndescription = \"g\"\n";
    let errors = errors_of(
        OntologyBuilder::new()
            .with_core()
            .add_source(
                "e.toml",
                ext("\n[[class]]\nid = \"Widget\"\ndescription = \"w\"\n"),
            )
            .add_source("p.toml", pack)
            .build(),
    );
    assert!(matches!(
        &errors.errors()[0].kind,
        LoadErrorKind::UnknownClass(c) if c == "Widget"
    ));
}
