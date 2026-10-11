//! Statements (n-ary typed facts with provenance) and their validation.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::model::{Cardinality, Datatype, Ontology, Property, Range};
use crate::value::{
    is_currency_code, is_email, is_url, parse_date, parse_datetime, Decimal, EntityRef, Money,
    RawMoney, RawValue, Value,
};

/// Which kind of extractor produced a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractorKind {
    /// Deterministic rule (regex, table header, document structure).
    Rule,
    /// GLiNER2 span/schema extraction.
    Gliner,
    /// Grounded LLM extraction constrained by an ontology slice.
    Llm,
    /// Stated or confirmed by the user.
    User,
}

impl ExtractorKind {
    /// Lower-case name (`rule`, `gliner`, `llm`, `user`).
    pub fn label(&self) -> &'static str {
        match self {
            ExtractorKind::Rule => "rule",
            ExtractorKind::Gliner => "gliner",
            ExtractorKind::Llm => "llm",
            ExtractorKind::User => "user",
        }
    }
}

/// The extractor and its version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Extractor {
    /// Extractor kind.
    pub kind: ExtractorKind,
    /// Extractor version (model id, rule-set version, app version for user input).
    pub version: String,
}

/// A half-open character range `[start, end)` in the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextSpan {
    /// First character (inclusive).
    pub start: usize,
    /// End character (exclusive).
    pub end: usize,
}

/// Where a statement came from (PROV-O shaped). Mandatory on every statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Source locator: document path or URI (`note://id`, `calendar://task/id`, ...).
    pub source: String,
    /// Index generation of the source the statement was extracted from.
    pub generation: u64,
    /// 1-based page number, when the source is paged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// Character span in the source (or page) text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<TextSpan>,
    /// The extractor that produced the statement.
    pub extractor: Extractor,
    /// Extractor confidence in `[0, 1]`.
    pub confidence: f64,
    /// When the statement was extracted.
    pub extracted_at: DateTime<Utc>,
}

/// An unvalidated statement as produced by an extractor: one n-ary fact about one subject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    /// Stable statement id (makes statements addressable and citable).
    pub id: String,
    /// Class of the subject described by the statement.
    pub class: String,
    /// The entity the statement is about, when it has been resolved to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<EntityRef>,
    /// Property values. A `Many` property may carry a [`RawValue::List`].
    #[serde(default)]
    pub properties: BTreeMap<String, RawValue>,
    /// Version of the source (core, pack or extension) defining `class` that the statement
    /// was extracted under. Checked with caret semantics against the loaded version.
    pub ontology_version: Version,
    /// When the fact became true. Defaults to the extraction time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<DateTime<Utc>>,
    /// Provenance. A statement without provenance is rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

/// A statement that passed validation. Only [`Ontology::validate`] constructs it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidStatement {
    id: String,
    class: String,
    subject: Option<EntityRef>,
    properties: BTreeMap<String, Vec<Value>>,
    ontology_version: Version,
    valid_from: Option<DateTime<Utc>>,
    provenance: Provenance,
}

impl ValidStatement {
    /// Statement id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Subject class.
    pub fn class(&self) -> &str {
        &self.class
    }

    /// Resolved subject, if any.
    pub fn subject(&self) -> Option<&EntityRef> {
        self.subject.as_ref()
    }

    /// Typed property values; `One` properties have exactly one value.
    pub fn properties(&self) -> &BTreeMap<String, Vec<Value>> {
        &self.properties
    }

    /// Values of one property (empty if absent).
    pub fn values(&self, property: &str) -> &[Value] {
        self.properties
            .get(property)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Ontology version the statement was extracted under.
    pub fn ontology_version(&self) -> &Version {
        &self.ontology_version
    }

    /// Declared start of validity, if any.
    pub fn valid_from(&self) -> Option<DateTime<Utc>> {
        self.valid_from
    }

    /// When the fact became true: `valid_from`, else the extraction time.
    pub fn effective_from(&self) -> DateTime<Utc> {
        self.valid_from.unwrap_or(self.provenance.extracted_at)
    }

    /// Provenance.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// One reason a statement was rejected. Violations are dropped and counted by callers,
/// never coerced into valid values.
#[derive(Debug, Clone, PartialEq, Serialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Violation {
    /// The statement has no provenance.
    #[error("statement has no provenance")]
    MissingProvenance,
    /// Provenance is present but malformed.
    #[error("invalid provenance: {reason}")]
    InvalidProvenance {
        /// What is wrong.
        reason: String,
    },
    /// The statement id is empty or contains whitespace/control characters.
    #[error("invalid statement id `{id}`")]
    InvalidId {
        /// The id.
        id: String,
    },
    /// The statement was extracted under a version of the defining source that the loaded
    /// version cannot read.
    #[error("statement version {statement} is not compatible with `{defined_by}` {ontology}")]
    IncompatibleVersion {
        /// Id of the source that defines the statement's class.
        defined_by: String,
        /// Version on the statement.
        statement: String,
        /// Loaded version of that source.
        ontology: String,
    },
    /// The class is not defined.
    #[error("unknown class `{class}`")]
    UnknownClass {
        /// The class id.
        class: String,
    },
    /// The subject reference names a class unrelated to the statement class.
    #[error("subject class `{subject_class}` is not related to statement class `{class}`")]
    SubjectClassMismatch {
        /// Statement class.
        class: String,
        /// Class on the subject reference.
        subject_class: String,
    },
    /// The property is not defined.
    #[error("unknown property `{property}`")]
    UnknownProperty {
        /// The property id.
        property: String,
    },
    /// The property exists but does not apply to the class.
    #[error("property `{property}` does not apply to `{class}` (domain: {})", domain.join(", "))]
    NotInDomain {
        /// The statement class.
        class: String,
        /// The property id.
        property: String,
        /// The property's domain.
        domain: Vec<String>,
    },
    /// A `One` property was given several values.
    #[error("property `{property}` takes one value, got {count}")]
    CardinalityExceeded {
        /// The property id.
        property: String,
        /// Number of values supplied.
        count: usize,
    },
    /// A required property is missing.
    #[error("required property `{property}` is missing")]
    MissingRequired {
        /// The property id.
        property: String,
    },
    /// The value has the wrong shape for the range (for example text for a relation).
    #[error("property `{property}` expects {expected}, got {found}")]
    TypeMismatch {
        /// The property id.
        property: String,
        /// Expected range.
        expected: String,
        /// Shape that was supplied.
        found: String,
    },
    /// The lexical form does not parse as the range datatype.
    #[error("property `{property}`: `{value}` is not a valid {expected}")]
    InvalidValue {
        /// The property id.
        property: String,
        /// Expected datatype.
        expected: String,
        /// The offending lexical form.
        value: String,
    },
    /// The value does not match the property's regex pattern.
    #[error("property `{property}`: `{value}` does not match pattern `{pattern}`")]
    PatternMismatch {
        /// The property id.
        property: String,
        /// The pattern.
        pattern: String,
        /// The offending value.
        value: String,
    },
    /// The value is not one of the enum values.
    #[error("property `{property}`: `{value}` is not one of {}", allowed.join(", "))]
    NotInEnum {
        /// The property id.
        property: String,
        /// The offending value.
        value: String,
        /// Allowed values.
        allowed: Vec<String>,
    },
    /// An entity reference names a class outside the property range.
    #[error("property `{property}` expects a `{expected}`, got a reference to a `{found}`")]
    RangeClassMismatch {
        /// The property id.
        property: String,
        /// Range class.
        expected: String,
        /// Class on the reference.
        found: String,
    },
}

impl Violation {
    /// Stable machine-readable code for counting violations per class.
    pub fn code(&self) -> &'static str {
        match self {
            Violation::MissingProvenance => "missing_provenance",
            Violation::InvalidProvenance { .. } => "invalid_provenance",
            Violation::InvalidId { .. } => "invalid_id",
            Violation::IncompatibleVersion { .. } => "incompatible_version",
            Violation::UnknownClass { .. } => "unknown_class",
            Violation::SubjectClassMismatch { .. } => "subject_class_mismatch",
            Violation::UnknownProperty { .. } => "unknown_property",
            Violation::NotInDomain { .. } => "not_in_domain",
            Violation::CardinalityExceeded { .. } => "cardinality_exceeded",
            Violation::MissingRequired { .. } => "missing_required",
            Violation::TypeMismatch { .. } => "type_mismatch",
            Violation::InvalidValue { .. } => "invalid_value",
            Violation::PatternMismatch { .. } => "pattern_mismatch",
            Violation::NotInEnum { .. } => "not_in_enum",
            Violation::RangeClassMismatch { .. } => "range_class_mismatch",
        }
    }
}

fn check_provenance(provenance: &Provenance) -> Option<String> {
    if provenance.source.trim().is_empty() {
        return Some("source is empty".to_owned());
    }
    if provenance.extractor.version.trim().is_empty() {
        return Some("extractor version is empty".to_owned());
    }
    if !provenance.confidence.is_finite() || !(0.0..=1.0).contains(&provenance.confidence) {
        return Some(format!(
            "confidence must be within [0, 1], got {}",
            provenance.confidence
        ));
    }
    if provenance.page == Some(0) {
        return Some("page numbers are 1-based".to_owned());
    }
    if let Some(span) = provenance.span {
        if span.start > span.end {
            return Some(format!("span start {} > end {}", span.start, span.end));
        }
    }
    None
}

impl Ontology {
    /// Validates a statement against the ontology.
    ///
    /// Checks provenance, version compatibility with the source that defines the class
    /// (caret semantics: a statement written under `1.2.0` is readable by `1.x` with
    /// `x >= 2`), class, property domains,
    /// value ranges and datatypes, patterns, enum membership, cardinality and required
    /// properties. All violations are reported, not just the first.
    pub fn validate(&self, statement: &Statement) -> Result<ValidStatement, Vec<Violation>> {
        let mut violations = Vec::new();

        match &statement.provenance {
            None => violations.push(Violation::MissingProvenance),
            Some(provenance) => {
                if let Some(reason) = check_provenance(provenance) {
                    violations.push(Violation::InvalidProvenance { reason });
                }
            }
        }
        if statement.id.trim().is_empty()
            || statement
                .id
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            violations.push(Violation::InvalidId {
                id: statement.id.clone(),
            });
        }
        // Statements record the version of the source (core, pack or extension) that
        // defines their class; each source is versioned independently.
        let owner = self
            .class(&statement.class)
            .and_then(|class| self.source(&class.source));
        let (owner_id, owner_version) = match owner {
            Some(source) => (source.id.as_str(), &source.version),
            None => (self.id.as_str(), &self.version),
        };
        let compatible = VersionReq::parse(&format!("^{}", statement.ontology_version))
            .map(|req| req.matches(owner_version))
            .unwrap_or(false);
        if !compatible {
            violations.push(Violation::IncompatibleVersion {
                defined_by: owner_id.to_owned(),
                statement: statement.ontology_version.to_string(),
                ontology: owner_version.to_string(),
            });
        }

        if self.class(&statement.class).is_none() {
            violations.push(Violation::UnknownClass {
                class: statement.class.clone(),
            });
            return Err(violations);
        }
        if let Some(subject_class) = statement.subject.as_ref().and_then(|s| s.class.as_ref()) {
            let related = self.is_subclass_of(subject_class, &statement.class)
                || self.is_subclass_of(&statement.class, subject_class);
            if !related {
                violations.push(Violation::SubjectClassMismatch {
                    class: statement.class.clone(),
                    subject_class: subject_class.clone(),
                });
            }
        }

        let mut properties = BTreeMap::new();
        for (name, raw) in &statement.properties {
            let Some(property) = self.property(name) else {
                violations.push(Violation::UnknownProperty {
                    property: name.clone(),
                });
                continue;
            };
            if !self.applies_to(property, &statement.class) {
                violations.push(Violation::NotInDomain {
                    class: statement.class.clone(),
                    property: name.clone(),
                    domain: property.domain.clone(),
                });
                continue;
            }
            let items: Vec<&RawValue> = match raw {
                RawValue::List(items) => items.iter().collect(),
                single => vec![single],
            };
            if property.cardinality == Cardinality::One && items.len() > 1 {
                violations.push(Violation::CardinalityExceeded {
                    property: name.clone(),
                    count: items.len(),
                });
                continue;
            }
            let mut values: Vec<Value> = Vec::with_capacity(items.len());
            for item in items {
                match self.parse_value(property, item) {
                    Ok(value) => {
                        if !values.contains(&value) {
                            values.push(value);
                        }
                    }
                    Err(violation) => violations.push(violation),
                }
            }
            if !values.is_empty() {
                properties.insert(name.clone(), values);
            }
        }

        for property in self.properties_of(&statement.class) {
            let supplied = statement
                .properties
                .get(&property.id)
                .is_some_and(|raw| !matches!(raw, RawValue::List(items) if items.is_empty()));
            if property.required && !supplied {
                violations.push(Violation::MissingRequired {
                    property: property.id.clone(),
                });
            }
        }

        match (&statement.provenance, violations.is_empty()) {
            (Some(provenance), true) => Ok(ValidStatement {
                id: statement.id.clone(),
                class: statement.class.clone(),
                subject: statement.subject.clone(),
                properties,
                ontology_version: statement.ontology_version.clone(),
                valid_from: statement.valid_from,
                provenance: provenance.clone(),
            }),
            _ => Err(violations),
        }
    }

    fn parse_value(&self, property: &Property, raw: &RawValue) -> Result<Value, Violation> {
        let mismatch = || Violation::TypeMismatch {
            property: property.id.clone(),
            expected: property.range.to_string(),
            found: raw.kind().to_owned(),
        };
        let invalid = |value: &str| Violation::InvalidValue {
            property: property.id.clone(),
            expected: property.range.to_string(),
            value: value.to_owned(),
        };
        let datatype = match &property.range {
            Range::Class(range) => {
                let RawValue::Entity(reference) = raw else {
                    return Err(mismatch());
                };
                if reference.id.trim().is_empty() {
                    return Err(invalid(&reference.id));
                }
                if let Some(class) = &reference.class {
                    if !self.is_subclass_of(class, range) {
                        return Err(Violation::RangeClassMismatch {
                            property: property.id.clone(),
                            expected: range.clone(),
                            found: class.clone(),
                        });
                    }
                }
                return Ok(Value::Entity(reference.clone()));
            }
            Range::Datatype(datatype) => datatype,
        };
        let value = match (datatype, raw) {
            (Datatype::String, RawValue::Text(text)) => Value::Text(text.clone()),
            (Datatype::Boolean, RawValue::Boolean(b)) => Value::Boolean(*b),
            (Datatype::Boolean, RawValue::Text(text)) => match text.as_str() {
                "true" => Value::Boolean(true),
                "false" => Value::Boolean(false),
                other => return Err(invalid(other)),
            },
            (Datatype::Integer, RawValue::Integer(i)) => Value::Integer(*i),
            (Datatype::Integer, RawValue::Text(text)) => {
                Value::Integer(text.parse::<i64>().map_err(|_| invalid(text))?)
            }
            (Datatype::Decimal, RawValue::Integer(i)) => {
                Value::Decimal(i.to_string().parse().map_err(|_| invalid(&i.to_string()))?)
            }
            (Datatype::Decimal, RawValue::Float(f)) => {
                let text = f.to_string();
                if !f.is_finite() {
                    return Err(invalid(&text));
                }
                Value::Decimal(text.parse().map_err(|_| invalid(&text))?)
            }
            (Datatype::Decimal, RawValue::Text(text)) => {
                Value::Decimal(text.parse::<Decimal>().map_err(|_| invalid(text))?)
            }
            (Datatype::Money, RawValue::Money(RawMoney { amount, currency })) => {
                money(amount, currency).ok_or_else(|| invalid(&format!("{amount} {currency}")))?
            }
            (Datatype::Money, RawValue::Text(text)) => {
                let parsed = text
                    .split_once(' ')
                    .and_then(|(amount, currency)| money(amount, currency));
                parsed.ok_or_else(|| invalid(text))?
            }
            (Datatype::Date, RawValue::Text(text)) => {
                Value::Date(parse_date(text).ok_or_else(|| invalid(text))?)
            }
            (Datatype::DateTime, RawValue::Text(text)) => {
                Value::DateTime(parse_datetime(text).ok_or_else(|| invalid(text))?)
            }
            (Datatype::Url, RawValue::Text(text)) if is_url(text) => Value::Url(text.clone()),
            (Datatype::Email, RawValue::Text(text)) if is_email(text) => Value::Email(text.clone()),
            (Datatype::Url | Datatype::Email, RawValue::Text(text)) => return Err(invalid(text)),
            (Datatype::Enum(allowed), RawValue::Text(text)) => {
                if !allowed.contains(text) {
                    return Err(Violation::NotInEnum {
                        property: property.id.clone(),
                        value: text.clone(),
                        allowed: allowed.clone(),
                    });
                }
                Value::Enum(text.clone())
            }
            _ => return Err(mismatch()),
        };
        if let (Some(pattern), Value::Text(text) | Value::Url(text) | Value::Email(text)) = (
            self.property_position(&property.id)
                .and_then(|i| self.anchored_patterns.get(i))
                .and_then(Option::as_ref),
            &value,
        ) {
            if !pattern.is_match(text) {
                return Err(Violation::PatternMismatch {
                    property: property.id.clone(),
                    pattern: property.pattern.clone().unwrap_or_default(),
                    value: text.clone(),
                });
            }
        }
        Ok(value)
    }
}

fn money(amount: &str, currency: &str) -> Option<Value> {
    if !is_currency_code(currency) {
        return None;
    }
    let amount = amount.parse::<Decimal>().ok()?;
    Some(Value::Money(Money {
        amount,
        currency: currency.to_owned(),
    }))
}
