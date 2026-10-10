//! Turtle export: OWL goldens, statement goldens with PROV-O, escaping, coverage.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{at, statement, valid, with_research};
use shodh_ontology::{
    EntityRef, Extractor, ExtractorKind, Ontology, OntologyBuilder, Provenance, RawValue,
    StatementExportOptions, TextSpan,
};

const SMALL: &str = r#"[core]
id = "t.core"
version = "1.0.0"
label = "Test ontology"
prefix = "t"
namespace = "urn:t:core#"

[[class]]
id = "Thing"
description = "Root."

[[class]]
id = "Org"
label = "Organization"
description = "A \"company\"."
identity_keys = [["taxNo"]]
equivalent_to = ["https://schema.org/Organization"]

[[class]]
id = "Bill"
description = "An invoice."

[[property]]
id = "taxNo"
description = "Tax number."
domain = "Org"
range = "String"
cardinality = "one"
required = true
pattern = '[0-9]{2}\d'

[[property]]
id = "issuer"
description = "Who issued it."
domain = "Bill"
range = "Org"
cardinality = "one"

[[property]]
id = "total"
description = "Total."
domain = "Bill"
range = "Money"
cardinality = "one"

[[property]]
id = "issuedOn"
description = "Issue date."
domain = "Bill"
range = "Date"
cardinality = "one"

[[property]]
id = "state"
description = "State."
domain = ["Bill", "Org"]
range = "Enum"
values = ["open", "paid"]
cardinality = "many"
"#;

const ONTOLOGY_GOLDEN: &str = r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix schema: <https://schema.org/> .
@prefix t: <urn:t:core#> .

<urn:t:core> a owl:Ontology ;
    rdfs:label "Test ontology" ;
    owl:versionInfo "1.0.0" .

t:Thing a owl:Class ;
    rdfs:label "Thing"@en ;
    rdfs:comment "Root."@en .

t:Org a owl:Class ;
    rdfs:label "Organization"@en ;
    rdfs:comment "A \"company\"."@en ;
    rdfs:subClassOf t:Thing ;
    rdfs:subClassOf [ a owl:Restriction ; owl:onProperty t:taxNo ; owl:cardinality "1"^^xsd:nonNegativeInteger ] ;
    owl:equivalentClass <https://schema.org/Organization> ;
    owl:hasKey ( t:taxNo ) .

t:Bill a owl:Class ;
    rdfs:label "Bill"@en ;
    rdfs:comment "An invoice."@en ;
    rdfs:subClassOf t:Thing ;
    rdfs:subClassOf [ a owl:Restriction ; owl:onProperty t:issuer ; owl:maxCardinality "1"^^xsd:nonNegativeInteger ] ;
    rdfs:subClassOf [ a owl:Restriction ; owl:onProperty t:total ; owl:maxCardinality "1"^^xsd:nonNegativeInteger ] ;
    rdfs:subClassOf [ a owl:Restriction ; owl:onProperty t:issuedOn ; owl:maxCardinality "1"^^xsd:nonNegativeInteger ] .

t:taxNo a owl:DatatypeProperty ;
    rdfs:label "taxNo"@en ;
    rdfs:comment "Tax number."@en ;
    rdfs:domain t:Org ;
    rdfs:range [ a rdfs:Datatype ; owl:onDatatype xsd:string ; owl:withRestrictions ( [ xsd:pattern "[0-9]{2}\\d" ] ) ] .

t:issuer a owl:ObjectProperty ;
    rdfs:label "issuer"@en ;
    rdfs:comment "Who issued it."@en ;
    rdfs:domain t:Bill ;
    rdfs:range t:Org .

t:total a owl:ObjectProperty ;
    rdfs:label "total"@en ;
    rdfs:comment "Total."@en ;
    rdfs:domain t:Bill ;
    rdfs:range schema:MonetaryAmount .

t:issuedOn a owl:DatatypeProperty ;
    rdfs:label "issuedOn"@en ;
    rdfs:comment "Issue date."@en ;
    rdfs:domain t:Bill ;
    rdfs:range xsd:date .

t:state a owl:DatatypeProperty ;
    rdfs:label "state"@en ;
    rdfs:comment "State."@en ;
    rdfs:domain [ a owl:Class ; owl:unionOf ( t:Bill t:Org ) ] ;
    rdfs:range [ a rdfs:Datatype ; owl:oneOf ( "open" "paid" ) ] .
"#;

const STATEMENTS_GOLDEN: &str = r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix schema: <https://schema.org/> .
@prefix prov: <http://www.w3.org/ns/prov#> .
@prefix sprov: <urn:shodh:prov#> .
@prefix t: <urn:t:core#> .

<urn:shodh:statement:bill%2F7%23a> a t:Bill, prov:Entity ;
    t:issuer <urn:shodh:entity:org%2Facme> ;
    t:total [ a schema:MonetaryAmount ; schema:value "1250.5"^^xsd:decimal ; schema:currency "INR" ] ;
    t:issuedOn "2026-09-30"^^xsd:date ;
    t:state "open", "paid" ;
    sprov:ontologyVersion "1.0.0" ;
    prov:wasDerivedFrom <urn:shodh:source:C%3A%2Fdocs%2Fbills%2F7.pdf> ;
    prov:generatedAtTime "2026-10-03T09:30:00+00:00"^^xsd:dateTime ;
    prov:wasGeneratedBy [ a prov:Activity ; prov:endedAtTime "2026-10-03T09:30:00+00:00"^^xsd:dateTime ; prov:wasAssociatedWith [ a prov:SoftwareAgent ; sprov:extractorKind "llm" ; sprov:extractorVersion "qwen3-8b@2026-09" ] ] ;
    sprov:generation "2"^^xsd:nonNegativeInteger ;
    sprov:page "1"^^xsd:positiveInteger ;
    sprov:charStart "10"^^xsd:nonNegativeInteger ;
    sprov:charEnd "42"^^xsd:nonNegativeInteger ;
    sprov:confidence "0.875"^^xsd:decimal .
"#;

fn small() -> Ontology {
    OntologyBuilder::new()
        .add_source("small.toml", SMALL)
        .build()
        .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn ontology_turtle_golden() {
    assert_eq!(small().to_turtle(), ONTOLOGY_GOLDEN);
}

#[test]
fn statement_turtle_golden_with_prov() {
    let o = small();
    let mut s = statement(
        "bill/7#a",
        "Bill",
        vec![
            (
                "issuer",
                RawValue::Entity(EntityRef::typed("org/acme", "Org")),
            ),
            ("total", RawValue::money("1250.50", "INR")),
            ("issuedOn", RawValue::text("2026-09-30")),
            ("state", RawValue::List(vec!["open".into(), "paid".into()])),
        ],
    );
    s.provenance = Some(Provenance {
        source: "C:/docs/bills/7.pdf".to_owned(),
        generation: 2,
        page: Some(1),
        span: Some(TextSpan { start: 10, end: 42 }),
        extractor: Extractor {
            kind: ExtractorKind::Llm,
            version: "qwen3-8b@2026-09".to_owned(),
        },
        confidence: 0.875,
        extracted_at: at("2026-10-03T09:30:00Z"),
    });
    let valid = valid(&o, &s);
    assert_eq!(
        o.statements_to_turtle(&[valid], &StatementExportOptions::default()),
        STATEMENTS_GOLDEN
    );
}

#[test]
fn user_statements_escape_text_and_use_prov_person() {
    let o = with_research();
    let mut s = statement(
        "note-1",
        "Note",
        vec![("noteText", RawValue::text("Line 1\n\"quoted\" \\ tab\t"))],
    );
    s.subject = Some(EntityRef::new("person varun"));
    s.valid_from = Some(at("2026-10-01T00:00:00Z"));
    if let Some(p) = s.provenance.as_mut() {
        p.source = "note://abc 1".to_owned();
        p.extractor.kind = ExtractorKind::User;
        p.page = None;
        p.span = None;
    }
    let turtle = o.statements_to_turtle(&[valid(&o, &s)], &StatementExportOptions::default());
    assert!(
        turtle.contains(r#"shodh:noteText "Line 1\n\"quoted\" \\ tab\t" ;"#),
        "{turtle}"
    );
    assert!(turtle.contains("sprov:about <urn:shodh:entity:person%20varun> ;"));
    assert!(turtle.contains("prov:wasDerivedFrom <note://abc%201> ;"));
    assert!(turtle.contains(r#"[ a prov:Person ; sprov:extractorKind "user""#));
    assert!(turtle.contains(r#"sprov:validFrom "2026-10-01T00:00:00+00:00"^^xsd:dateTime"#));
    assert!(!turtle.contains("sprov:page"));
}

#[test]
fn full_ontology_export_covers_every_term() {
    let o = with_research();
    let turtle = o.to_turtle();
    assert!(turtle.contains("@prefix research: <urn:shodh:ontology:research#> ."));
    assert!(turtle.contains("owl:imports <urn:shodh:ontology:core>"));
    for class in o.classes() {
        let prefix = &o.source(&class.source).unwrap().namespace.prefix;
        assert!(
            turtle.contains(&format!("\n{prefix}:{} a owl:Class ;", class.id)),
            "{} missing",
            class.id
        );
    }
    for property in o.properties() {
        let prefix = &o.source(&property.source).unwrap().namespace.prefix;
        assert!(
            turtle.contains(&format!("\n{prefix}:{} a owl:", property.id)),
            "{} missing",
            property.id
        );
    }
    assert!(turtle.contains("research:Paper a owl:Class ;"));
    assert!(turtle.contains("rdfs:subClassOf shodh:Document"));
    assert!(turtle.contains("owl:hasKey ( shodh:invoiceNumber shodh:issuedBy )"));
    // Regex backslashes are escaped inside literals.
    assert!(
        turtle.contains(r#"xsd:pattern "10\\.[0-9]{4,9}/[^\\s]+""#),
        "doi pattern"
    );
    assert_eq!(turtle, o.to_turtle());
}
