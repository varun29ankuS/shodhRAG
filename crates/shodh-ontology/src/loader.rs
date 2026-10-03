//! TOML loader: parses core, pack and extension sources, merges them under the
//! add-only rule, validates every reference and compiles the indexed [`Ontology`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use regex::Regex;
use semver::{Version, VersionReq};
use serde::Deserialize;
use toml::Spanned;

use crate::builtin;
use crate::error::{LoadError, LoadErrorKind, LoadErrors, SourceLocation};
use crate::model::{
    Cardinality, Class, Datatype, Dynamics, Expiry, Layer, Namespace, Ontology, Property, Range,
    SourceInfo,
};

/// The root class every class descends from. It must be defined by the core source.
pub const ROOT_CLASS: &str = "Thing";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    core: Option<RawHeader>,
    pack: Option<RawHeader>,
    extension: Option<RawHeader>,
    #[serde(default)]
    class: Vec<RawClass>,
    #[serde(default)]
    property: Vec<RawProperty>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHeader {
    id: Spanned<String>,
    version: Spanned<String>,
    label: Option<String>,
    prefix: Spanned<String>,
    namespace: Spanned<String>,
    #[serde(default)]
    requires: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClass {
    id: Spanned<String>,
    label: Option<String>,
    description: String,
    parent: Option<Spanned<String>>,
    #[serde(default)]
    identity_keys: Vec<Vec<String>>,
    #[serde(default)]
    cue_terms: Vec<String>,
    #[serde(default)]
    cue_patterns: Vec<String>,
    dynamics: Option<RawDynamics>,
    #[serde(default)]
    equivalent_to: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDynamics {
    decay_half_life_days: Option<f64>,
    #[serde(default)]
    reinforcement: f64,
    expires_after: Option<RawExpiry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExpiry {
    days: Option<f64>,
    property: Option<String>,
    grace_days: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawCardinality {
    One,
    Many,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProperty {
    id: Spanned<String>,
    label: Option<String>,
    description: String,
    domain: RawDomain,
    range: String,
    values: Option<Vec<String>>,
    cardinality: RawCardinality,
    #[serde(default)]
    temporal: bool,
    #[serde(default)]
    required: bool,
    pattern: Option<String>,
    #[serde(default)]
    pattern_is_cue: bool,
    #[serde(default)]
    equivalent_to: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawDomain {
    One(String),
    Many(Vec<String>),
}

impl RawDomain {
    fn to_vec(&self) -> Vec<String> {
        match self {
            RawDomain::One(class) => vec![class.clone()],
            RawDomain::Many(classes) => classes.clone(),
        }
    }
}

struct ParsedSource {
    name: String,
    text: String,
    layer: Layer,
    header: RawHeader,
    classes: Vec<RawClass>,
    properties: Vec<RawProperty>,
}

impl ParsedSource {
    fn loc(&self, offset: usize) -> SourceLocation {
        SourceLocation::from_offset(&self.name, &self.text, offset)
    }
}

/// Builds an [`Ontology`] from a core source, optional packs and workspace extensions.
///
/// Sources may be added in any order; they are merged core first, then packs, then
/// extensions, each group in the order added. Every error is collected and returned
/// together by [`OntologyBuilder::build`].
#[derive(Debug, Default)]
pub struct OntologyBuilder {
    sources: Vec<(String, String)>,
    errors: Vec<LoadError>,
}

impl OntologyBuilder {
    /// An empty builder. Add a core source with [`OntologyBuilder::with_core`] or
    /// [`OntologyBuilder::add_source`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the built-in core ontology.
    pub fn with_core(self) -> Self {
        self.add_source(builtin::CORE_NAME, builtin::CORE_TOML)
    }

    /// Adds a built-in pack by name (for example `research`).
    pub fn with_builtin_pack(mut self, name: &str) -> Self {
        match builtin::pack(name) {
            Some((file, text)) => self.add_source(file, text),
            None => {
                self.errors.push(LoadError::global(
                    LoadErrorKind::UnknownBuiltinPack(name.to_owned()),
                ));
                self
            }
        }
    }

    /// Adds a source from text. `name` is used in error locations (usually a file path).
    pub fn add_source(mut self, name: impl Into<String>, text: impl Into<String>) -> Self {
        self.sources.push((name.into(), text.into()));
        self
    }

    /// Reads and adds one TOML file.
    pub fn add_file(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let name = path.display().to_string();
        match std::fs::read_to_string(path) {
            Ok(text) => self.add_source(name, text),
            Err(error) => {
                self.errors.push(LoadError::at(
                    SourceLocation {
                        file: name,
                        line: 1,
                        column: 1,
                    },
                    LoadErrorKind::Io(error.to_string()),
                ));
                self
            }
        }
    }

    /// Adds every `*.toml` file in a directory (not recursive), sorted by file name.
    pub fn add_dir(mut self, dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                self.errors.push(LoadError::at(
                    SourceLocation {
                        file: dir.display().to_string(),
                        line: 1,
                        column: 1,
                    },
                    LoadErrorKind::Io(error.to_string()),
                ));
                return self;
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    if path.is_file() && path.extension().is_some_and(|ext| ext == "toml") {
                        paths.push(path);
                    }
                }
                Err(error) => self.errors.push(LoadError::at(
                    SourceLocation {
                        file: dir.display().to_string(),
                        line: 1,
                        column: 1,
                    },
                    LoadErrorKind::Io(error.to_string()),
                )),
            }
        }
        paths.sort();
        for path in paths {
            self = self.add_file(path);
        }
        self
    }

    /// Parses, merges, validates and compiles all sources.
    pub fn build(self) -> Result<Ontology, LoadErrors> {
        let mut errors = self.errors;
        let mut parsed = Vec::new();
        for (name, text) in self.sources {
            match parse_source(name, text) {
                Ok(source) => parsed.push(source),
                Err(error) => errors.push(error),
            }
        }
        if !errors.is_empty() {
            return Err(LoadErrors(errors));
        }
        // Stable sort keeps insertion order within a layer.
        parsed.sort_by_key(|s| s.layer);
        let ontology = Compiler::new(&parsed, &mut errors).compile();
        match ontology {
            Some(ontology) if errors.is_empty() => Ok(ontology),
            _ => Err(LoadErrors(errors)),
        }
    }
}

impl Ontology {
    /// Loads the built-in core ontology.
    pub fn builtin() -> Result<Self, LoadErrors> {
        OntologyBuilder::new().with_core().build()
    }

    /// Loads the built-in core ontology plus the named built-in packs.
    pub fn builtin_with_packs(packs: &[&str]) -> Result<Self, LoadErrors> {
        packs
            .iter()
            .fold(OntologyBuilder::new().with_core(), |builder, pack| {
                builder.with_builtin_pack(pack)
            })
            .build()
    }
}

fn parse_source(name: String, text: String) -> Result<ParsedSource, LoadError> {
    let raw: RawFile = toml::from_str(&text).map_err(|error| {
        let offset = error.span().map(|span| span.start).unwrap_or(0);
        LoadError::at(
            SourceLocation::from_offset(&name, &text, offset),
            LoadErrorKind::Syntax(error.message().trim().to_owned()),
        )
    })?;
    let mut headers = Vec::new();
    if let Some(header) = raw.core {
        headers.push((Layer::Core, header));
    }
    if let Some(header) = raw.pack {
        headers.push((Layer::Pack, header));
    }
    if let Some(header) = raw.extension {
        headers.push((Layer::Extension, header));
    }
    if headers.len() != 1 {
        return Err(LoadError::at(
            SourceLocation::from_offset(&name, &text, 0),
            LoadErrorKind::MissingHeader,
        ));
    }
    let Some((layer, header)) = headers.pop() else {
        return Err(LoadError::at(
            SourceLocation::from_offset(&name, &text, 0),
            LoadErrorKind::MissingHeader,
        ));
    };
    Ok(ParsedSource {
        name,
        text,
        layer,
        header,
        classes: raw.class,
        properties: raw.property,
    })
}

fn is_class_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.all(|c| c.is_ascii_alphanumeric())
}

fn is_property_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_alphanumeric())
}

fn is_prefix(prefix: &str) -> bool {
    let mut chars = prefix.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_namespace_iri(iri: &str) -> bool {
    (iri.starts_with("https://") || iri.starts_with("http://") || iri.starts_with("urn:"))
        && (iri.ends_with('#') || iri.ends_with('/'))
        && !iri.chars().any(|c| c.is_whitespace() || "<>\"{}|^`\\".contains(c))
}

/// Compiles a pattern so it must match the whole value.
pub(crate) fn anchored(pattern: &str) -> Result<Regex, regex::Error> {
    Regex::new(&format!("^(?:{pattern})$"))
}

/// Compiles a pattern for finding values inside running text.
fn scanning(pattern: &str) -> Result<Regex, regex::Error> {
    Regex::new(&format!(r"\b(?:{pattern})\b"))
}

/// Builds one case-insensitive matcher from cue terms (word-bounded) and cue patterns.
fn cue_matcher(terms: &[String], patterns: &[String]) -> Result<Option<Regex>, regex::Error> {
    let mut alternatives = Vec::new();
    for term in terms {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        let escaped = term
            .split_whitespace()
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join(r"\s+");
        let starts_word = term.chars().next().is_some_and(|c| c.is_alphanumeric());
        let ends_word = term.chars().last().is_some_and(|c| c.is_alphanumeric());
        alternatives.push(format!(
            "(?i:{}{}{})",
            if starts_word { r"\b" } else { "" },
            escaped,
            if ends_word { r"\b" } else { "" }
        ));
    }
    for pattern in patterns {
        alternatives.push(format!("(?:{pattern})"));
    }
    if alternatives.is_empty() {
        return Ok(None);
    }
    Regex::new(&alternatives.join("|")).map(Some)
}

fn parse_range(raw: &str, values: Option<&Vec<String>>) -> Option<Range> {
    let datatype = match raw {
        "String" => Datatype::String,
        "Boolean" => Datatype::Boolean,
        "Integer" => Datatype::Integer,
        "Decimal" => Datatype::Decimal,
        "Money" => Datatype::Money,
        "Date" => Datatype::Date,
        "DateTime" => Datatype::DateTime,
        "Url" => Datatype::Url,
        "Email" => Datatype::Email,
        "Enum" => Datatype::Enum(values.cloned().unwrap_or_default()),
        _ => return None,
    };
    Some(Range::Datatype(datatype))
}

fn check_days(value: f64, what: &str, allow_zero: bool) -> Result<(), String> {
    if !value.is_finite() || value < 0.0 || (!allow_zero && value == 0.0) {
        let bound = if allow_zero { ">= 0" } else { "> 0" };
        return Err(format!("{what} must be a finite number {bound}, got {value}"));
    }
    Ok(())
}

/// Term id -> (definition location, index of the defining source in merge order).
type TermTable = BTreeMap<String, (SourceLocation, usize)>;

struct Compiler<'a> {
    sources: &'a [ParsedSource],
    errors: &'a mut Vec<LoadError>,
    /// Dynamics declared explicitly on a class; others inherit from their parent.
    declared_dynamics: BTreeMap<String, Dynamics>,
    /// Malformed `[class.dynamics]` tables, reported with the other dynamics errors.
    dynamics_shape_errors: BTreeMap<String, String>,
}

impl<'a> Compiler<'a> {
    fn new(sources: &'a [ParsedSource], errors: &'a mut Vec<LoadError>) -> Self {
        Self {
            sources,
            errors,
            declared_dynamics: BTreeMap::new(),
            dynamics_shape_errors: BTreeMap::new(),
        }
    }

    fn error(&mut self, location: SourceLocation, kind: LoadErrorKind) {
        self.errors.push(LoadError::at(location, kind));
    }

    fn compile(mut self) -> Option<Ontology> {
        let infos = self.headers()?;
        let (class_terms, property_terms) = self.register_terms();
        let classes = self.classes(&class_terms);
        let properties = self.properties(&class_terms, &property_terms);
        if !self.errors.is_empty() {
            return None;
        }

        let class_index: BTreeMap<String, usize> = classes
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();
        let property_index: BTreeMap<String, usize> = properties
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();

        let core = infos.first()?;
        let mut ontology = Ontology {
            id: core.id.clone(),
            version: core.version.clone(),
            sources: infos.clone(),
            classes,
            properties,
            class_index,
            property_index,
            applicable: Vec::new(),
            cue_matchers: Vec::new(),
            anchored_patterns: Vec::new(),
            scan_patterns: Vec::new(),
        };
        if !self.check_hierarchy(&ontology) {
            return None;
        }
        ontology.applicable = ontology
            .classes
            .iter()
            .map(|class| {
                ontology
                    .properties
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| ontology.applies_to(p, &class.id))
                    .map(|(i, _)| i)
                    .collect()
            })
            .collect();
        self.check_required_ownership(&ontology);
        self.resolve_dynamics(&mut ontology);
        self.check_identity_keys(&ontology);
        self.compile_patterns(&mut ontology);
        if self.errors.is_empty() {
            Some(ontology)
        } else {
            None
        }
    }

    fn headers(&mut self) -> Option<Vec<SourceInfo>> {
        let core_count = self
            .sources
            .iter()
            .filter(|s| s.layer == Layer::Core)
            .count();
        if core_count != 1 {
            self.errors
                .push(LoadError::global(LoadErrorKind::CoreCount(core_count)));
            return None;
        }
        let mut infos: Vec<SourceInfo> = Vec::new();
        for source in self.sources {
            let header = &source.header;
            let location = source.loc(header.id.span().start);
            let version = match Version::parse(header.version.get_ref()) {
                Ok(version) => version,
                Err(error) => {
                    self.error(
                        source.loc(header.version.span().start),
                        LoadErrorKind::InvalidVersion {
                            value: header.version.get_ref().clone(),
                            reason: error.to_string(),
                        },
                    );
                    continue;
                }
            };
            if let Some(first) = infos.iter().find(|i| &i.id == header.id.get_ref()) {
                let first = first.location.clone();
                self.error(
                    location.clone(),
                    LoadErrorKind::DuplicateSource {
                        id: header.id.get_ref().clone(),
                        first,
                    },
                );
            }
            let prefix = header.prefix.get_ref();
            if !is_prefix(prefix) {
                self.error(
                    source.loc(header.prefix.span().start),
                    LoadErrorKind::InvalidNamespace(format!(
                        "prefix `{prefix}` must start with a letter and contain only letters, digits, `-` or `_`"
                    )),
                );
            } else if let Some(other) = infos.iter().find(|i| &i.namespace.prefix == prefix) {
                let other_id = other.id.clone();
                self.error(
                    source.loc(header.prefix.span().start),
                    LoadErrorKind::InvalidNamespace(format!(
                        "prefix `{prefix}` is already used by `{other_id}`"
                    )),
                );
            }
            let iri = header.namespace.get_ref();
            if !is_namespace_iri(iri) {
                self.error(
                    source.loc(header.namespace.span().start),
                    LoadErrorKind::InvalidNamespace(format!(
                        "`{iri}` must be an http(s) or urn IRI ending in `#` or `/`"
                    )),
                );
            }
            infos.push(SourceInfo {
                id: header.id.get_ref().clone(),
                version,
                layer: source.layer,
                label: header
                    .label
                    .clone()
                    .unwrap_or_else(|| header.id.get_ref().clone()),
                namespace: Namespace {
                    prefix: prefix.clone(),
                    iri: iri.clone(),
                },
                location,
            });
        }
        if infos.len() != self.sources.len() {
            return None;
        }
        // Requirements may only point at sources merged earlier.
        for (index, source) in self.sources.iter().enumerate() {
            for (id, requirement) in &source.header.requires {
                let location = source.loc(source.header.id.span().start);
                let req = match VersionReq::parse(requirement) {
                    Ok(req) => req,
                    Err(error) => {
                        self.error(
                            location,
                            LoadErrorKind::InvalidVersion {
                                value: requirement.clone(),
                                reason: error.to_string(),
                            },
                        );
                        continue;
                    }
                };
                let found = infos[..index].iter().find(|i| &i.id == id);
                let problem = match found {
                    None => Some("it is not loaded before this source".to_owned()),
                    Some(info) if !req.matches(&info.version) => {
                        Some(format!("version {} is loaded", info.version))
                    }
                    Some(_) => None,
                };
                if let Some(found) = problem {
                    self.error(
                        location,
                        LoadErrorKind::UnmetRequirement {
                            id: id.clone(),
                            requirement: requirement.clone(),
                            found,
                        },
                    );
                }
            }
        }
        Some(infos)
    }

    /// Registers every term id, enforcing the naming rules and the add-only merge rule.
    fn register_terms(&mut self) -> (TermTable, TermTable) {
        let mut all: BTreeMap<String, SourceLocation> = BTreeMap::new();
        let mut classes = TermTable::new();
        let mut properties = TermTable::new();
        for (index, source) in self.sources.iter().enumerate() {
            let class_ids = source.classes.iter().map(|c| (&c.id, true));
            let property_ids = source.properties.iter().map(|p| (&p.id, false));
            for (id, is_class) in class_ids.chain(property_ids) {
                let location = source.loc(id.span().start);
                let name = id.get_ref();
                let (valid, kind, rule) = if is_class {
                    (
                        is_class_id(name) && !Datatype::NAMES.contains(&name.as_str()),
                        "class",
                        "must be UpperCamelCase ASCII letters/digits and not a datatype name",
                    )
                } else {
                    (
                        is_property_id(name),
                        "property",
                        "must be lowerCamelCase ASCII letters/digits",
                    )
                };
                if !valid {
                    self.error(
                        location,
                        LoadErrorKind::InvalidId {
                            kind,
                            id: name.clone(),
                            rule,
                        },
                    );
                    continue;
                }
                if let Some(first) = all.get(name) {
                    let first = first.clone();
                    self.error(
                        location,
                        LoadErrorKind::Redefinition {
                            id: name.clone(),
                            first,
                        },
                    );
                    continue;
                }
                all.insert(name.clone(), location.clone());
                let table = if is_class {
                    &mut classes
                } else {
                    &mut properties
                };
                table.insert(name.clone(), (location, index));
            }
        }
        (classes, properties)
    }

    /// Resolves a class reference, which must be defined by this source or an earlier one.
    fn class_ref(
        &mut self,
        terms: &TermTable,
        name: &str,
        source_index: usize,
        location: &SourceLocation,
    ) -> bool {
        match terms.get(name) {
            Some((_, defined_in)) if *defined_in <= source_index => true,
            _ => {
                self.error(
                    location.clone(),
                    LoadErrorKind::UnknownClass(name.to_owned()),
                );
                false
            }
        }
    }

    fn classes(&mut self, terms: &TermTable) -> Vec<Class> {
        let mut out = Vec::new();
        for (index, source) in self.sources.iter().enumerate() {
            let source_id = source.header.id.get_ref().clone();
            for raw in &source.classes {
                let id = raw.id.get_ref();
                let location = source.loc(raw.id.span().start);
                match terms.get(id) {
                    Some((defined, _)) if *defined == location => {}
                    _ => continue, // invalid or duplicate: already reported
                }
                let parent = match &raw.parent {
                    Some(parent) => {
                        let parent_location = source.loc(parent.span().start);
                        self.class_ref(terms, parent.get_ref(), index, &parent_location);
                        Some(parent.get_ref().clone())
                    }
                    None if id == ROOT_CLASS => None,
                    None => {
                        self.class_ref(terms, ROOT_CLASS, index, &location);
                        Some(ROOT_CLASS.to_owned())
                    }
                };
                if id == ROOT_CLASS && (raw.parent.is_some() || source.layer != Layer::Core) {
                    self.error(
                        location.clone(),
                        LoadErrorKind::InheritanceCycle(format!(
                            "`{ROOT_CLASS}` is the root class; it must be defined by the core without a parent"
                        )),
                    );
                }
                let dynamics = raw.dynamics.as_ref().map(|d| Dynamics {
                    decay_half_life_days: d.decay_half_life_days,
                    reinforcement: d.reinforcement,
                    expires_after: d.expires_after.as_ref().map(|e| match &e.property {
                        Some(property) => Expiry::AtProperty {
                            property: property.clone(),
                            grace_days: e.grace_days.unwrap_or(0.0),
                        },
                        // A missing `days` is reported by `validate_dynamics_shape`.
                        None => Expiry::After {
                            days: e.days.unwrap_or(0.0),
                        },
                    }),
                });
                if let Some(dynamics) = dynamics {
                    self.declared_dynamics.insert(id.clone(), dynamics);
                }
                if let Some(Err(reason)) = raw.dynamics.as_ref().map(validate_dynamics_shape) {
                    self.dynamics_shape_errors.insert(id.clone(), reason);
                }
                out.push(Class {
                    id: id.clone(),
                    label: raw.label.clone().unwrap_or_else(|| id.clone()),
                    description: raw.description.trim().to_owned(),
                    parent,
                    identity_keys: raw.identity_keys.clone(),
                    cue_terms: raw.cue_terms.clone(),
                    cue_patterns: raw.cue_patterns.clone(),
                    // Resolved (declared or inherited) in `resolve_dynamics`.
                    dynamics: Dynamics::default(),
                    equivalent_to: raw.equivalent_to.clone(),
                    source: source_id.clone(),
                    location,
                });
            }
        }
        out
    }

    fn properties(&mut self, classes: &TermTable, terms: &TermTable) -> Vec<Property> {
        let mut out = Vec::new();
        for (index, source) in self.sources.iter().enumerate() {
            let source_id = source.header.id.get_ref().clone();
            for raw in &source.properties {
                let id = raw.id.get_ref();
                let location = source.loc(raw.id.span().start);
                match terms.get(id) {
                    Some((defined, _)) if *defined == location => {}
                    _ => continue,
                }
                let invalid = |reason: String| LoadErrorKind::InvalidProperty {
                    property: id.clone(),
                    reason,
                };
                let domain = raw.domain.to_vec();
                if domain.is_empty() {
                    self.error(
                        location.clone(),
                        invalid("domain must name at least one class".to_owned()),
                    );
                }
                let unique: BTreeSet<&String> = domain.iter().collect();
                if unique.len() != domain.len() {
                    self.error(
                        location.clone(),
                        invalid("domain lists a class twice".to_owned()),
                    );
                }
                for class in &domain {
                    self.class_ref(classes, class, index, &location);
                }
                let range = match parse_range(&raw.range, raw.values.as_ref()) {
                    Some(range) => range,
                    None => {
                        if !classes.contains_key(&raw.range) {
                            self.error(
                                location.clone(),
                                LoadErrorKind::UnknownRange(raw.range.clone()),
                            );
                        } else {
                            self.class_ref(classes, &raw.range, index, &location);
                        }
                        Range::Class(raw.range.clone())
                    }
                };
                match &range {
                    Range::Datatype(Datatype::Enum(values)) => {
                        let distinct: BTreeSet<&String> = values.iter().collect();
                        if values.is_empty() || values.iter().any(|v| v.trim().is_empty()) {
                            self.error(
                                location.clone(),
                                invalid(
                                    "range Enum needs a non-empty `values` list of non-empty strings"
                                        .to_owned(),
                                ),
                            );
                        } else if distinct.len() != values.len() {
                            self.error(
                                location.clone(),
                                invalid("Enum `values` must be distinct".to_owned()),
                            );
                        }
                    }
                    _ if raw.values.is_some() => self.error(
                        location.clone(),
                        invalid("`values` is only allowed with range = \"Enum\"".to_owned()),
                    ),
                    _ => {}
                }
                if let Some(pattern) = &raw.pattern {
                    let accepts = matches!(&range, Range::Datatype(d) if d.accepts_pattern());
                    if !accepts {
                        self.error(
                            location.clone(),
                            invalid(format!(
                                "`pattern` is only allowed with String, Url or Email ranges, not {range}"
                            )),
                        );
                    } else if let Err(error) = anchored(pattern).and_then(|_| scanning(pattern)) {
                        self.error(
                            location.clone(),
                            LoadErrorKind::InvalidPattern {
                                pattern: pattern.clone(),
                                reason: error.to_string(),
                            },
                        );
                    }
                }
                let cardinality = match raw.cardinality {
                    RawCardinality::One => Cardinality::One,
                    RawCardinality::Many => Cardinality::Many,
                };
                if raw.pattern_is_cue && raw.pattern.is_none() {
                    self.error(
                        location.clone(),
                        invalid("`pattern_is_cue = true` requires a `pattern`".to_owned()),
                    );
                }
                if raw.temporal && cardinality != Cardinality::One {
                    self.error(
                        location.clone(),
                        invalid(
                            "`temporal = true` requires cardinality = \"one\" (supersede semantics)"
                                .to_owned(),
                        ),
                    );
                }
                out.push(Property {
                    id: id.clone(),
                    label: raw.label.clone().unwrap_or_else(|| id.clone()),
                    description: raw.description.trim().to_owned(),
                    domain,
                    range,
                    cardinality,
                    temporal: raw.temporal,
                    required: raw.required,
                    pattern: raw.pattern.clone(),
                    pattern_is_cue: raw.pattern_is_cue,
                    equivalent_to: raw.equivalent_to.clone(),
                    source: source_id.clone(),
                    location,
                });
            }
        }
        out
    }

    fn check_hierarchy(&mut self, ontology: &Ontology) -> bool {
        let mut ok = true;
        for class in &ontology.classes {
            let mut seen = BTreeSet::new();
            let mut path = vec![class.id.as_str()];
            let mut current = class.parent.as_deref();
            seen.insert(class.id.as_str());
            while let Some(parent) = current {
                path.push(parent);
                if !seen.insert(parent) {
                    self.error(
                        class.location.clone(),
                        LoadErrorKind::InheritanceCycle(path.join(" -> ")),
                    );
                    ok = false;
                    break;
                }
                current = ontology.class(parent).and_then(|c| c.parent.as_deref());
            }
        }
        ok
    }

    fn check_required_ownership(&mut self, ontology: &Ontology) {
        for property in &ontology.properties {
            if !property.required {
                continue;
            }
            for domain in &property.domain {
                if let Some(class) = ontology.class(domain) {
                    if class.source != property.source {
                        self.error(
                            property.location.clone(),
                            LoadErrorKind::RequiredOnForeignClass {
                                property: property.id.clone(),
                                class: class.id.clone(),
                                owner: class.source.clone(),
                            },
                        );
                    }
                }
            }
        }
    }

    fn resolve_dynamics(&mut self, ontology: &mut Ontology) {
        let order: Vec<usize> = {
            // Parents before children: sort by depth.
            let mut by_depth: Vec<(usize, usize)> = ontology
                .classes
                .iter()
                .enumerate()
                .map(|(i, c)| (ontology.ancestors(&c.id).len(), i))
                .collect();
            by_depth.sort();
            by_depth.into_iter().map(|(_, i)| i).collect()
        };
        for index in order {
            let class = &ontology.classes[index];
            let declared = self.declared_dynamics.get(&class.id).cloned();
            let is_declared = declared.is_some();
            let resolved = if let Some(declared) = declared {
                declared
            } else {
                class
                    .parent
                    .as_deref()
                    .and_then(|p| ontology.class(p))
                    .map(|p| p.dynamics.clone())
                    .unwrap_or_default()
            };
            if is_declared {
                let checked = match self.dynamics_shape_errors.get(&class.id) {
                    Some(reason) => Err(reason.clone()),
                    None => validate_dynamics_values(ontology, &class.id, &resolved),
                };
                if let Err(reason) = checked {
                    let location = class.location.clone();
                    let id = class.id.clone();
                    self.error(
                        location,
                        LoadErrorKind::InvalidDynamics { class: id, reason },
                    );
                }
            }
            ontology.classes[index].dynamics = resolved;
        }
    }

    fn check_identity_keys(&mut self, ontology: &Ontology) {
        for class in &ontology.classes {
            for key in &class.identity_keys {
                if key.is_empty() {
                    self.error(
                        class.location.clone(),
                        LoadErrorKind::PropertyNotApplicable {
                            class: class.id.clone(),
                            property: "<empty identity key>".to_owned(),
                        },
                    );
                }
                for property in key {
                    let applies = ontology
                        .property(property)
                        .is_some_and(|p| ontology.applies_to(p, &class.id));
                    if !applies {
                        self.error(
                            class.location.clone(),
                            LoadErrorKind::PropertyNotApplicable {
                                class: class.id.clone(),
                                property: property.clone(),
                            },
                        );
                    }
                }
            }
        }
    }

    fn compile_patterns(&mut self, ontology: &mut Ontology) {
        let mut cue_matchers = Vec::with_capacity(ontology.classes.len());
        for class in &ontology.classes {
            match cue_matcher(&class.cue_terms, &class.cue_patterns) {
                Ok(matcher) => cue_matchers.push(matcher),
                Err(error) => {
                    self.error(
                        class.location.clone(),
                        LoadErrorKind::InvalidPattern {
                            pattern: class.cue_patterns.join(" | "),
                            reason: error.to_string(),
                        },
                    );
                    cue_matchers.push(None);
                }
            }
        }
        let mut anchored_patterns = Vec::with_capacity(ontology.properties.len());
        let mut scan_patterns = Vec::with_capacity(ontology.properties.len());
        for property in &ontology.properties {
            match &property.pattern {
                Some(pattern) => match (anchored(pattern), scanning(pattern)) {
                    (Ok(full), Ok(scan)) => {
                        anchored_patterns.push(Some(full));
                        scan_patterns.push(property.pattern_is_cue.then_some(scan));
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        self.error(
                            property.location.clone(),
                            LoadErrorKind::InvalidPattern {
                                pattern: pattern.clone(),
                                reason: error.to_string(),
                            },
                        );
                        anchored_patterns.push(None);
                        scan_patterns.push(None);
                    }
                },
                None => {
                    anchored_patterns.push(None);
                    scan_patterns.push(None);
                }
            }
        }
        ontology.cue_matchers = cue_matchers;
        ontology.anchored_patterns = anchored_patterns;
        ontology.scan_patterns = scan_patterns;
    }
}

fn validate_dynamics_shape(raw: &RawDynamics) -> Result<(), String> {
    if let Some(expiry) = &raw.expires_after {
        match (&expiry.property, expiry.days) {
            (Some(_), Some(_)) => {
                return Err(
                    "expires_after takes either `days` or `property` (+ `grace_days`), not both"
                        .to_owned(),
                )
            }
            (None, None) => {
                return Err("expires_after needs `days` or `property`".to_owned());
            }
            (None, Some(_)) if expiry.grace_days.is_some() => {
                return Err("`grace_days` is only valid together with `property`".to_owned());
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_dynamics_values(
    ontology: &Ontology,
    class: &str,
    dynamics: &Dynamics,
) -> Result<(), String> {
    if let Some(half_life) = dynamics.decay_half_life_days {
        check_days(half_life, "decay_half_life_days", false)?;
    }
    if !dynamics.reinforcement.is_finite() || !(0.0..=1.0).contains(&dynamics.reinforcement) {
        return Err(format!(
            "reinforcement must be within [0, 1], got {}",
            dynamics.reinforcement
        ));
    }
    match &dynamics.expires_after {
        Some(Expiry::After { days }) => check_days(*days, "expires_after.days", false)?,
        Some(Expiry::AtProperty {
            property,
            grace_days,
        }) => {
            check_days(*grace_days, "expires_after.grace_days", true)?;
            let Some(p) = ontology.property(property) else {
                return Err(format!("expires_after.property `{property}` is not defined"));
            };
            if !ontology.applies_to(p, class) {
                return Err(format!(
                    "expires_after.property `{property}` does not apply to `{class}`"
                ));
            }
            if !matches!(
                p.range,
                Range::Datatype(Datatype::Date) | Range::Datatype(Datatype::DateTime)
            ) || p.cardinality != Cardinality::One
            {
                return Err(format!(
                    "expires_after.property `{property}` must be a single-valued Date or DateTime"
                ));
            }
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_survive_nested_tables() -> Result<(), String> {
        let text = "[core]
id = \"t\"
version = \"1.0.0\"
prefix = \"t\"
namespace = \"urn:t#\"

[[class]]
id = \"Thing\"
description = \"root\"

[[class]]
id = \"Child\"
parent = \"Thing\"
description = \"c\"
";
        let raw: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let child = raw.class.get(1).ok_or("missing class")?;
        let location = SourceLocation::from_offset("f.toml", text, child.id.span().start);
        assert_eq!(location.line, 12);
        let parent = child.parent.as_ref().map(|p| p.span().start);
        assert_eq!(
            parent.map(|o| SourceLocation::from_offset("f.toml", text, o).line),
            Some(13)
        );
        Ok(())
    }

    #[test]
    fn domain_accepts_string_or_list() -> Result<(), String> {
        let one: RawProperty = toml::from_str(
            "id = \"p\"
description = \"d\"
domain = \"A\"
range = \"String\"
cardinality = \"one\"
",
        )
        .map_err(|e| e.to_string())?;
        assert_eq!(one.domain.to_vec(), vec!["A".to_owned()]);
        let many: RawProperty = toml::from_str(
            "id = \"p\"
description = \"d\"
domain = [\"A\", \"B\"]
range = \"String\"
cardinality = \"many\"
",
        )
        .map_err(|e| e.to_string())?;
        assert_eq!(many.domain.to_vec(), vec!["A".to_owned(), "B".to_owned()]);
        Ok(())
    }
}
