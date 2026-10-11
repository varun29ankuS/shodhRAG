//! The compiled, indexed ontology model.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use regex::Regex;
use semver::Version;
use serde::Serialize;

use crate::error::SourceLocation;

/// Which layer of the ontology a source belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// The built-in core ontology. Exactly one per compiled ontology.
    Core,
    /// An opt-in domain pack (for example `research`).
    Pack,
    /// A workspace extension authored by the user or proposed by the agent and approved.
    Extension,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Layer::Core => "core",
            Layer::Pack => "pack",
            Layer::Extension => "extension",
        })
    }
}

/// An IRI namespace used when exporting terms of one source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Namespace {
    /// Turtle prefix (for example `shodh`).
    pub prefix: String,
    /// Namespace IRI ending in `#` or `/`.
    pub iri: String,
}

/// Metadata of one loaded source (core, pack or extension).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceInfo {
    /// Source id, for example `shodh.core` or `shodh.research`.
    pub id: String,
    /// Semantic version of this source.
    pub version: Version,
    /// Layer.
    pub layer: Layer,
    /// Human-readable label.
    pub label: String,
    /// Namespace for exported terms.
    pub namespace: Namespace,
    /// Where the header was declared.
    pub location: SourceLocation,
}

/// Single- or multi-valued property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cardinality {
    /// At most one current value per subject (a functional property).
    One,
    /// Any number of values per subject.
    Many,
}

/// Literal datatypes a property may range over.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "type", content = "values")]
pub enum Datatype {
    /// Free text. Combine with a property `pattern` for regex-constrained text.
    String,
    /// `true` / `false`.
    Boolean,
    /// 64-bit signed integer.
    Integer,
    /// Exact decimal (`-?digits[.digits]`), never a float.
    Decimal,
    /// Exact decimal amount plus ISO 4217 currency code.
    Money,
    /// Calendar date, ISO 8601 `YYYY-MM-DD`.
    Date,
    /// Instant with UTC offset, RFC 3339 / ISO 8601 (`2026-10-03T10:00:00+05:30`).
    DateTime,
    /// Absolute URL with a scheme.
    Url,
    /// E-mail address.
    Email,
    /// One of a closed list of values.
    Enum(Vec<String>),
}

impl Datatype {
    /// The name used in TOML (`range = "Date"`).
    pub fn name(&self) -> &'static str {
        match self {
            Datatype::String => "String",
            Datatype::Boolean => "Boolean",
            Datatype::Integer => "Integer",
            Datatype::Decimal => "Decimal",
            Datatype::Money => "Money",
            Datatype::Date => "Date",
            Datatype::DateTime => "DateTime",
            Datatype::Url => "Url",
            Datatype::Email => "Email",
            Datatype::Enum(_) => "Enum",
        }
    }

    /// Whether a `pattern` may further constrain this datatype's lexical form.
    pub fn accepts_pattern(&self) -> bool {
        matches!(self, Datatype::String | Datatype::Url | Datatype::Email)
    }

    pub(crate) const NAMES: [&'static str; 10] = [
        "String", "Boolean", "Integer", "Decimal", "Money", "Date", "DateTime", "Url", "Email",
        "Enum",
    ];
}

/// What values of a property may be.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", content = "target")]
pub enum Range {
    /// A reference to an entity of this class (or a subclass): an object property.
    Class(String),
    /// A literal value: a datatype property.
    Datatype(Datatype),
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Range::Class(class) => write!(f, "{class}"),
            Range::Datatype(Datatype::Enum(values)) => write!(f, "Enum[{}]", values.join("|")),
            Range::Datatype(datatype) => f.write_str(datatype.name()),
        }
    }
}

/// When a memory of this class stops being current.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expiry {
    /// A fixed time after the statement became valid.
    After {
        /// Days after `valid_from`.
        days: f64,
    },
    /// A date or date-time property of the statement plus a grace period
    /// (for example a task expires 30 days after its `dueOn`).
    AtProperty {
        /// Property holding the deadline (range `Date` or `DateTime`).
        property: String,
        /// Days after the deadline.
        grace_days: f64,
    },
}

/// Memory dynamics for statements of a class, consumed by the memory module.
///
/// Strength `s(t) = s0 * 0.5^(age_days / decay_half_life_days)`; each retrieval or
/// re-assertion adds `reinforcement * (1 - s)`; a statement past its expiry is no longer
/// current but is kept as history.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Dynamics {
    /// Half-life of recall strength in days. `None` means no decay (records, documents).
    pub decay_half_life_days: Option<f64>,
    /// Fraction of the remaining headroom restored on reinforcement, in `[0, 1]`.
    pub reinforcement: f64,
    /// Expiry rule. `None` means the statement never expires on its own.
    pub expires_after: Option<Expiry>,
}

impl Default for Dynamics {
    fn default() -> Self {
        Self {
            decay_half_life_days: None,
            reinforcement: 0.0,
            expires_after: None,
        }
    }
}

/// A class of things statements can describe.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Class {
    /// Identifier, `UpperCamelCase`.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Definition shown to users and LLMs.
    pub description: String,
    /// Single parent class. `None` only for the root class `Thing`.
    pub parent: Option<String>,
    /// Identity keys declared on this class (inherited keys are resolved by
    /// [`Ontology::identity_keys_of`]). Two mentions are the same entity when, for any
    /// one key, every property in that key has an equal value on both.
    pub identity_keys: Vec<Vec<String>>,
    /// Case-insensitive cue words or phrases that suggest the class is present in text.
    pub cue_terms: Vec<String>,
    /// Regular expressions that suggest the class is present in text.
    pub cue_patterns: Vec<String>,
    /// Effective memory dynamics (declared here or inherited from the nearest ancestor).
    pub dynamics: Dynamics,
    /// External IRIs this class is equivalent to (schema.org, FIBO, ...).
    pub equivalent_to: Vec<String>,
    /// Id of the source that defines the class.
    pub source: String,
    /// Where the class is defined.
    pub location: SourceLocation,
}

/// A property of a class: an attribute (datatype range) or a relation (class range).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Property {
    /// Identifier, `lowerCamelCase`.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Definition shown to users and LLMs.
    pub description: String,
    /// Classes the property applies to (it also applies to their subclasses).
    pub domain: Vec<String>,
    /// Value type.
    pub range: Range,
    /// One or many values per subject.
    pub cardinality: Cardinality,
    /// Temporal functional property: a newer value supersedes the older one, which is kept
    /// as history with a `valid_to`. Only allowed with [`Cardinality::One`].
    pub temporal: bool,
    /// Every statement of a domain class must carry the property.
    pub required: bool,
    /// Regex the whole lexical value must match (only for String, Url and Email ranges).
    pub pattern: Option<String>,
    /// Whether finding the pattern in text selects the domain classes when slicing.
    /// Only for distinctive patterns (GSTIN, DOI), never for loose ones (postal codes).
    pub pattern_is_cue: bool,
    /// External IRIs this property is equivalent to.
    pub equivalent_to: Vec<String>,
    /// Id of the source that defines the property.
    pub source: String,
    /// Where the property is defined.
    pub location: SourceLocation,
}

impl Property {
    /// Whether this is an object property (its range is a class).
    pub fn is_relation(&self) -> bool {
        matches!(self.range, Range::Class(_))
    }
}

/// The compiled ontology: core plus packs plus workspace extensions, indexed for lookup.
#[derive(Debug, Clone)]
pub struct Ontology {
    pub(crate) id: String,
    pub(crate) version: Version,
    pub(crate) sources: Vec<SourceInfo>,
    pub(crate) classes: Vec<Class>,
    pub(crate) properties: Vec<Property>,
    pub(crate) class_index: BTreeMap<String, usize>,
    pub(crate) property_index: BTreeMap<String, usize>,
    /// Class index -> indices of applicable properties (own and inherited), declaration order.
    pub(crate) applicable: Vec<Vec<usize>>,
    /// Class index -> compiled cue matcher (terms and patterns), if any.
    pub(crate) cue_matchers: Vec<Option<Regex>>,
    /// Property index -> anchored validation regex.
    pub(crate) anchored_patterns: Vec<Option<Regex>>,
    /// Property index -> unanchored, word-bounded regex used for slicing
    /// (only for properties with `pattern_is_cue`).
    pub(crate) scan_patterns: Vec<Option<Regex>>,
}

impl Ontology {
    /// Id of the core ontology (for example `shodh.core`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Version of the core ontology. Statements record this version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// All loaded sources: the core first, then packs, then extensions.
    pub fn sources(&self) -> &[SourceInfo] {
        &self.sources
    }

    /// Loaded packs and extensions (everything except the core).
    pub fn packs(&self) -> impl Iterator<Item = &SourceInfo> {
        self.sources.iter().filter(|s| s.layer != Layer::Core)
    }

    /// All classes in load order.
    pub fn classes(&self) -> &[Class] {
        &self.classes
    }

    /// All properties in load order.
    pub fn properties(&self) -> &[Property] {
        &self.properties
    }

    /// Looks up a class by id.
    pub fn class(&self, id: &str) -> Option<&Class> {
        self.class_index.get(id).map(|&i| &self.classes[i])
    }

    /// Looks up a property by id.
    pub fn property(&self, id: &str) -> Option<&Property> {
        self.property_index.get(id).map(|&i| &self.properties[i])
    }

    /// The source a term came from.
    pub fn source(&self, id: &str) -> Option<&SourceInfo> {
        self.sources.iter().find(|s| s.id == id)
    }

    /// The class and its ancestors, nearest first. Empty if the class is unknown.
    pub fn ancestors(&self, class: &str) -> Vec<&Class> {
        let mut chain = Vec::new();
        let mut current = self.class(class);
        while let Some(c) = current {
            chain.push(c);
            current = c.parent.as_deref().and_then(|p| self.class(p));
        }
        chain
    }

    /// Whether `class` is `ancestor` or one of its subclasses.
    pub fn is_subclass_of(&self, class: &str, ancestor: &str) -> bool {
        self.ancestors(class).iter().any(|c| c.id == ancestor)
    }

    /// Direct and indirect subclasses of `class`, in load order (excluding `class`).
    pub fn descendants(&self, class: &str) -> Vec<&Class> {
        self.classes
            .iter()
            .filter(|c| c.id != class && self.is_subclass_of(&c.id, class))
            .collect()
    }

    /// Properties applicable to a class (own and inherited), in load order.
    /// Empty if the class is unknown.
    pub fn properties_of(&self, class: &str) -> Vec<&Property> {
        self.class_index
            .get(class)
            .map(|&i| {
                self.applicable[i]
                    .iter()
                    .map(|&p| &self.properties[p])
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether `property` applies to `class` (directly or via an ancestor).
    pub fn applies_to(&self, property: &Property, class: &str) -> bool {
        property
            .domain
            .iter()
            .any(|domain| self.is_subclass_of(class, domain))
    }

    /// Identity keys of a class including inherited ones, nearest class first.
    pub fn identity_keys_of(&self, class: &str) -> Vec<&[String]> {
        self.ancestors(class)
            .into_iter()
            .flat_map(|c| c.identity_keys.iter().map(Vec::as_slice))
            .collect()
    }

    /// Ids of all terms (classes and properties).
    pub fn term_ids(&self) -> BTreeSet<&str> {
        self.class_index
            .keys()
            .chain(self.property_index.keys())
            .map(String::as_str)
            .collect()
    }

    pub(crate) fn property_position(&self, id: &str) -> Option<usize> {
        self.property_index.get(id).copied()
    }
}
