//! OWL 2 / Turtle export of the ontology and Turtle export of statements with W3C PROV-O
//! provenance. Import is not implemented; OWL is an interchange format only.
//!
//! Mapping:
//! - class → `owl:Class` with `rdfs:label`, `rdfs:comment`, `rdfs:subClassOf`,
//!   `owl:equivalentClass`, one `owl:hasKey` per identity key;
//! - relation → `owl:ObjectProperty`; attribute → `owl:DatatypeProperty`
//!   (`Money` → `owl:ObjectProperty` ranging over `schema:MonetaryAmount`);
//! - multi-class domain → `owl:unionOf`; `Enum` → `owl:oneOf`; `pattern` → `xsd:pattern`
//!   datatype restriction;
//! - cardinality and `required` → `owl:Restriction` (`owl:cardinality 1` for one+required,
//!   `owl:maxCardinality 1` for one, `owl:minCardinality 1` for many+required) on each
//!   domain class.
//!
//! Not exported (no OWL counterpart): memory dynamics, cue terms/patterns and the
//! `temporal` flag.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::model::{Cardinality, Class, Datatype, Layer, Ontology, Property, Range};
use crate::statement::{ExtractorKind, ValidStatement};
use crate::value::Value;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const SCHEMA: &str = "https://schema.org/";
const PROV: &str = "http://www.w3.org/ns/prov#";
/// Namespace for Shodh-specific provenance terms not covered by PROV-O
/// (page, character span, generation, confidence, extractor kind and version).
pub const SHODH_PROV: &str = "urn:shodh:prov#";

/// IRI prefixes used when exporting statements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementExportOptions {
    /// Prefix for statement IRIs (statement id is appended, percent-encoded).
    pub statement_base: String,
    /// Prefix for entity IRIs (entity id is appended, percent-encoded).
    pub entity_base: String,
    /// Prefix for sources that are not already absolute IRIs (paths).
    pub source_base: String,
}

impl Default for StatementExportOptions {
    fn default() -> Self {
        Self {
            statement_base: "urn:shodh:statement:".to_owned(),
            entity_base: "urn:shodh:entity:".to_owned(),
            source_base: "urn:shodh:source:".to_owned(),
        }
    }
}

/// Escapes a string for a Turtle `"..."` literal.
fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn percent_encode_char(out: &mut String, c: char) {
    let mut buffer = [0u8; 4];
    for byte in c.encode_utf8(&mut buffer).bytes() {
        let _ = write!(out, "%{byte:02X}");
    }
}

/// Writes `<iri>`, percent-encoding characters Turtle forbids inside IRIREFs.
fn iri(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('<');
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() || "<>\"{}|^`\\".contains(c) {
            percent_encode_char(&mut out, c);
        } else {
            out.push(c);
        }
    }
    out.push('>');
    out
}

/// Appends an id to a base, percent-encoding everything except unreserved characters.
fn minted(base: &str, id: &str) -> String {
    let mut out = String::from(base);
    for c in id.chars() {
        if c.is_ascii_alphanumeric() || "-._~".contains(c) {
            out.push(c);
        } else {
            percent_encode_char(&mut out, c);
        }
    }
    iri(&out)
}

fn ontology_iri(namespace: &str) -> String {
    iri(namespace.trim_end_matches(['#', '/']))
}

struct Writer<'a> {
    ontology: &'a Ontology,
    prefixes: BTreeMap<&'a str, &'a str>,
}

impl<'a> Writer<'a> {
    fn new(ontology: &'a Ontology) -> Self {
        let prefixes = ontology
            .sources
            .iter()
            .map(|s| (s.id.as_str(), s.namespace.prefix.as_str()))
            .collect();
        Self { ontology, prefixes }
    }

    fn term(&self, source: &str, id: &str) -> String {
        match self.prefixes.get(source) {
            Some(prefix) => format!("{prefix}:{id}"),
            None => iri(id),
        }
    }

    fn class(&self, id: &str) -> String {
        self.ontology
            .class(id)
            .map(|c| self.term(&c.source, &c.id))
            .unwrap_or_else(|| iri(id))
    }

    fn property(&self, property: &Property) -> String {
        self.term(&property.source, &property.id)
    }

    fn header(&self, out: &mut String, extra: &[(&str, &str)]) {
        let mut prefixes: Vec<(&str, &str)> = vec![
            ("rdf", RDF),
            ("rdfs", RDFS),
            ("owl", OWL),
            ("xsd", XSD),
            ("schema", SCHEMA),
        ];
        prefixes.extend_from_slice(extra);
        for source in &self.ontology.sources {
            prefixes.push((&source.namespace.prefix, &source.namespace.iri));
        }
        for (prefix, namespace) in prefixes {
            let _ = writeln!(out, "@prefix {prefix}: {} .", iri(namespace));
        }
    }
}

fn datatype_iri(datatype: &Datatype) -> &'static str {
    match datatype {
        Datatype::String | Datatype::Email | Datatype::Enum(_) => "xsd:string",
        Datatype::Boolean => "xsd:boolean",
        Datatype::Integer => "xsd:integer",
        Datatype::Decimal => "xsd:decimal",
        Datatype::Money => "schema:MonetaryAmount",
        Datatype::Date => "xsd:date",
        Datatype::DateTime => "xsd:dateTime",
        Datatype::Url => "xsd:anyURI",
    }
}

impl Ontology {
    /// Exports the ontology as OWL 2 in Turtle syntax. Output is deterministic.
    pub fn to_turtle(&self) -> String {
        let writer = Writer::new(self);
        let mut out = String::new();
        writer.header(&mut out, &[]);

        let core_iri = self
            .sources
            .first()
            .map(|s| ontology_iri(&s.namespace.iri))
            .unwrap_or_default();
        for source in &self.sources {
            let _ = write!(
                out,
                "\n{} a owl:Ontology ;\n    rdfs:label {} ;\n    owl:versionInfo {}",
                ontology_iri(&source.namespace.iri),
                literal(&source.label),
                literal(&source.version.to_string())
            );
            if source.layer != Layer::Core {
                let _ = write!(out, " ;\n    owl:imports {core_iri}");
            }
            out.push_str(" .\n");
        }

        // Cardinality restrictions, grouped by domain class.
        let mut restrictions: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for property in &self.properties {
            let restriction = match (property.cardinality, property.required) {
                (Cardinality::One, true) => "owl:cardinality",
                (Cardinality::One, false) => "owl:maxCardinality",
                (Cardinality::Many, true) => "owl:minCardinality",
                (Cardinality::Many, false) => continue,
            };
            for domain in &property.domain {
                restrictions.entry(domain).or_default().push(format!(
                    "[ a owl:Restriction ; owl:onProperty {} ; {restriction} \"1\"^^xsd:nonNegativeInteger ]",
                    writer.property(property)
                ));
            }
        }

        for class in &self.classes {
            write_class(
                &writer,
                &mut out,
                class,
                restrictions.get(class.id.as_str()),
            );
        }
        for property in &self.properties {
            write_property(&writer, &mut out, property);
        }
        out
    }

    /// Exports validated statements as Turtle, each statement a `prov:Entity` typed with
    /// its ontology class and carrying W3C PROV-O provenance. Output is deterministic.
    pub fn statements_to_turtle(
        &self,
        statements: &[ValidStatement],
        options: &StatementExportOptions,
    ) -> String {
        let writer = Writer::new(self);
        let mut out = String::new();
        writer.header(&mut out, &[("prov", PROV), ("sprov", SHODH_PROV)]);
        for statement in statements {
            write_statement(&writer, &mut out, statement, options);
        }
        out
    }
}

fn write_class(
    writer: &Writer<'_>,
    out: &mut String,
    class: &Class,
    restrictions: Option<&Vec<String>>,
) {
    let mut lines = vec![
        "a owl:Class".to_owned(),
        format!("rdfs:label {}@en", literal(&class.label)),
        format!("rdfs:comment {}@en", literal(&class.description)),
    ];
    if let Some(parent) = &class.parent {
        lines.push(format!("rdfs:subClassOf {}", writer.class(parent)));
    }
    for restriction in restrictions.into_iter().flatten() {
        lines.push(format!("rdfs:subClassOf {restriction}"));
    }
    for equivalent in &class.equivalent_to {
        lines.push(format!("owl:equivalentClass {}", iri(equivalent)));
    }
    for key in &class.identity_keys {
        let members: Vec<String> = key
            .iter()
            .filter_map(|p| writer.ontology.property(p))
            .map(|p| writer.property(p))
            .collect();
        lines.push(format!("owl:hasKey ( {} )", members.join(" ")));
    }
    let _ = write!(
        out,
        "\n{} {} .\n",
        writer.term(&class.source, &class.id),
        lines.join(" ;\n    ")
    );
}

fn write_property(writer: &Writer<'_>, out: &mut String, property: &Property) {
    let object = matches!(
        property.range,
        Range::Class(_) | Range::Datatype(Datatype::Money)
    );
    let mut lines = vec![
        format!(
            "a {}",
            if object {
                "owl:ObjectProperty"
            } else {
                "owl:DatatypeProperty"
            }
        ),
        format!("rdfs:label {}@en", literal(&property.label)),
        format!("rdfs:comment {}@en", literal(&property.description)),
    ];
    let domain = match property.domain.as_slice() {
        [single] => writer.class(single),
        many => format!(
            "[ a owl:Class ; owl:unionOf ( {} ) ]",
            many.iter()
                .map(|d| writer.class(d))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    };
    lines.push(format!("rdfs:domain {domain}"));
    let range = match (&property.range, &property.pattern) {
        (Range::Class(class), _) => writer.class(class),
        (Range::Datatype(Datatype::Enum(values)), _) => format!(
            "[ a rdfs:Datatype ; owl:oneOf ( {} ) ]",
            values
                .iter()
                .map(|v| literal(v))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        (Range::Datatype(datatype), Some(pattern)) => format!(
            "[ a rdfs:Datatype ; owl:onDatatype {} ; owl:withRestrictions ( [ xsd:pattern {} ] ) ]",
            datatype_iri(datatype),
            literal(pattern)
        ),
        (Range::Datatype(datatype), None) => datatype_iri(datatype).to_owned(),
    };
    lines.push(format!("rdfs:range {range}"));
    for equivalent in &property.equivalent_to {
        lines.push(format!("owl:equivalentProperty {}", iri(equivalent)));
    }
    let _ = write!(
        out,
        "\n{} {} .\n",
        writer.property(property),
        lines.join(" ;\n    ")
    );
}

fn value_term(value: &Value, options: &StatementExportOptions) -> String {
    match value {
        Value::Text(text) | Value::Email(text) | Value::Enum(text) => literal(text),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => format!("{}^^xsd:integer", literal(&i.to_string())),
        Value::Decimal(d) => format!("{}^^xsd:decimal", literal(d.as_str())),
        Value::Money(money) => format!(
            "[ a schema:MonetaryAmount ; schema:value {}^^xsd:decimal ; schema:currency {} ]",
            literal(money.amount.as_str()),
            literal(&money.currency)
        ),
        Value::Date(date) => format!(
            "{}^^xsd:date",
            literal(&date.format("%Y-%m-%d").to_string())
        ),
        Value::DateTime(dt) => format!("{}^^xsd:dateTime", literal(&dt.to_rfc3339())),
        Value::Url(url) => format!("{}^^xsd:anyURI", literal(url)),
        Value::Entity(entity) => minted(&options.entity_base, &entity.id),
    }
}

fn source_iri(source: &str, options: &StatementExportOptions) -> String {
    let has_scheme = source.split_once(':').is_some_and(|(scheme, rest)| {
        scheme.len() > 1
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
            && !rest.is_empty()
    });
    if has_scheme {
        iri(source)
    } else {
        minted(&options.source_base, source)
    }
}

fn write_statement(
    writer: &Writer<'_>,
    out: &mut String,
    statement: &ValidStatement,
    options: &StatementExportOptions,
) {
    let provenance = statement.provenance();
    let mut lines = vec![format!(
        "a {}, prov:Entity",
        writer.class(statement.class())
    )];
    if let Some(subject) = statement.subject() {
        lines.push(format!(
            "sprov:about {}",
            minted(&options.entity_base, &subject.id)
        ));
    }
    for property in writer.ontology.properties_of(statement.class()) {
        let values = statement.values(&property.id);
        if values.is_empty() {
            continue;
        }
        let objects: Vec<String> = values.iter().map(|v| value_term(v, options)).collect();
        lines.push(format!(
            "{} {}",
            writer.property(property),
            objects.join(", ")
        ));
    }
    lines.push(format!(
        "sprov:ontologyVersion {}",
        literal(&statement.ontology_version().to_string())
    ));
    if let Some(valid_from) = statement.valid_from() {
        lines.push(format!(
            "sprov:validFrom {}^^xsd:dateTime",
            literal(&valid_from.to_rfc3339())
        ));
    }
    lines.push(format!(
        "prov:wasDerivedFrom {}",
        source_iri(&provenance.source, options)
    ));
    let extracted_at = literal(&provenance.extracted_at.to_rfc3339());
    lines.push(format!("prov:generatedAtTime {extracted_at}^^xsd:dateTime"));
    let agent_type = match provenance.extractor.kind {
        ExtractorKind::User => "prov:Person",
        _ => "prov:SoftwareAgent",
    };
    lines.push(format!(
        "prov:wasGeneratedBy [ a prov:Activity ; prov:endedAtTime {extracted_at}^^xsd:dateTime ; \
prov:wasAssociatedWith [ a {agent_type} ; sprov:extractorKind {} ; sprov:extractorVersion {} ] ]",
        literal(provenance.extractor.kind.label()),
        literal(&provenance.extractor.version)
    ));
    lines.push(format!(
        "sprov:generation {}^^xsd:nonNegativeInteger",
        literal(&provenance.generation.to_string())
    ));
    if let Some(page) = provenance.page {
        lines.push(format!(
            "sprov:page {}^^xsd:positiveInteger",
            literal(&page.to_string())
        ));
    }
    if let Some(span) = provenance.span {
        lines.push(format!(
            "sprov:charStart {}^^xsd:nonNegativeInteger",
            literal(&span.start.to_string())
        ));
        lines.push(format!(
            "sprov:charEnd {}^^xsd:nonNegativeInteger",
            literal(&span.end.to_string())
        ));
    }
    lines.push(format!(
        "sprov:confidence {}^^xsd:decimal",
        literal(&provenance.confidence.to_string())
    ));
    let _ = write!(
        out,
        "\n{} {} .\n",
        minted(&options.statement_base, statement.id()),
        lines.join(" ;\n    ")
    );
}
