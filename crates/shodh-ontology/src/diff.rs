//! Version compatibility between two compiled ontologies.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::Serialize;

use crate::model::{Class, Ontology, Property};

/// A class field that changed between versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassField {
    /// Label.
    Label,
    /// Description.
    Description,
    /// Parent class.
    Parent,
    /// Identity keys.
    IdentityKeys,
    /// Cue terms.
    CueTerms,
    /// Cue patterns.
    CuePatterns,
    /// Memory dynamics.
    Dynamics,
    /// External equivalences.
    EquivalentTo,
}

/// A property field that changed between versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyField {
    /// Label.
    Label,
    /// Description.
    Description,
    /// Domain.
    Domain,
    /// Range (including enum values).
    Range,
    /// Cardinality.
    Cardinality,
    /// Temporal flag.
    Temporal,
    /// Required flag.
    Required,
    /// Value pattern.
    Pattern,
    /// Whether the pattern is used as a slicing cue.
    PatternIsCue,
    /// External equivalences.
    EquivalentTo,
}

/// A changed term and which fields changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermChange<F> {
    /// Term id.
    pub id: String,
    /// Changed fields.
    pub fields: Vec<F>,
}

/// How a version change affects stored statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    /// No term changed.
    Identical,
    /// Only documentation changed (labels, descriptions, equivalences, dynamics).
    Cosmetic,
    /// Terms were added or matching/identity hints changed; existing statements stay valid.
    Additive,
    /// Terms were removed or constraints changed; existing statements may be invalid.
    Breaking,
}

/// Version movement of one source (core, pack or extension) and the severity of the
/// changes to the terms it defines.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceChange {
    /// Source id.
    pub source: String,
    /// Version in the old ontology (`None` if the source was added).
    pub from: Option<Version>,
    /// Version in the new ontology (`None` if the source was removed).
    pub to: Option<Version>,
    /// Severity of the changes to this source's terms.
    pub compatibility: Compatibility,
}

impl SourceChange {
    /// Whether this source's version moved enough for its changes: cosmetic changes need a
    /// higher version, additive changes a minor bump, breaking changes a major bump (a minor
    /// bump for `0.x`, following semver caret rules). A newly added source is always
    /// sufficient; a removed source never is (its statements can no longer be read).
    pub fn bump_sufficient(&self) -> bool {
        let (from, to) = match (&self.from, &self.to) {
            (None, Some(_)) => return true,
            (_, None) => return false,
            (Some(from), Some(to)) => (from, to),
        };
        match self.compatibility {
            Compatibility::Identical => to >= from,
            Compatibility::Cosmetic => to > from,
            Compatibility::Additive => (to.major, to.minor) > (from.major, from.minor),
            Compatibility::Breaking => {
                if from.major == 0 {
                    to.major > 0 || to.minor > from.minor
                } else {
                    to.major > from.major
                }
            }
        }
    }
}

/// Differences between two ontology versions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OntologyDiff {
    /// Old core version.
    pub from: Version,
    /// New core version.
    pub to: Version,
    /// Classes only in the new ontology.
    pub added_classes: Vec<String>,
    /// Classes only in the old ontology.
    pub removed_classes: Vec<String>,
    /// Classes present in both with changed fields.
    pub changed_classes: Vec<TermChange<ClassField>>,
    /// Properties only in the new ontology.
    pub added_properties: Vec<String>,
    /// Properties only in the old ontology.
    pub removed_properties: Vec<String>,
    /// Properties present in both with changed fields.
    pub changed_properties: Vec<TermChange<PropertyField>>,
    /// Per-source version movement and severity, sorted by source id. Every source of
    /// either ontology is listed.
    pub sources: Vec<SourceChange>,
    /// Overall compatibility (the most severe source change).
    pub compatibility: Compatibility,
    /// Classes whose documents must be re-extracted (a new generation), sorted.
    /// Includes subclasses of affected classes. Dynamics, labels, descriptions and
    /// equivalences do not require re-extraction.
    pub affected_classes: Vec<String>,
}

fn class_severity(field: &ClassField) -> Compatibility {
    match field {
        ClassField::Parent => Compatibility::Breaking,
        ClassField::IdentityKeys | ClassField::CueTerms | ClassField::CuePatterns => {
            Compatibility::Additive
        }
        ClassField::Label
        | ClassField::Description
        | ClassField::Dynamics
        | ClassField::EquivalentTo => Compatibility::Cosmetic,
    }
}

fn property_severity(field: &PropertyField) -> Compatibility {
    match field {
        PropertyField::Domain
        | PropertyField::Range
        | PropertyField::Cardinality
        | PropertyField::Temporal
        | PropertyField::Required
        | PropertyField::Pattern => Compatibility::Breaking,
        PropertyField::PatternIsCue => Compatibility::Additive,
        PropertyField::Label | PropertyField::Description | PropertyField::EquivalentTo => {
            Compatibility::Cosmetic
        }
    }
}

impl OntologyDiff {
    /// Computes the diff from `old` to `new`.
    pub fn between(old: &Ontology, new: &Ontology) -> Self {
        let old_classes: BTreeSet<&str> = old.classes.iter().map(|c| c.id.as_str()).collect();
        let new_classes: BTreeSet<&str> = new.classes.iter().map(|c| c.id.as_str()).collect();
        let old_props: BTreeSet<&str> = old.properties.iter().map(|p| p.id.as_str()).collect();
        let new_props: BTreeSet<&str> = new.properties.iter().map(|p| p.id.as_str()).collect();

        let added_classes = owned(new_classes.difference(&old_classes));
        let removed_classes = owned(old_classes.difference(&new_classes));
        let added_properties = owned(new_props.difference(&old_props));
        let removed_properties = owned(old_props.difference(&new_props));

        let changed_classes: Vec<TermChange<ClassField>> = old_classes
            .intersection(&new_classes)
            .filter_map(|id| {
                let fields = class_fields(old.class(id)?, new.class(id)?);
                (!fields.is_empty()).then(|| TermChange {
                    id: (*id).to_owned(),
                    fields,
                })
            })
            .collect();
        let changed_properties: Vec<TermChange<PropertyField>> = old_props
            .intersection(&new_props)
            .filter_map(|id| {
                let fields = property_fields(old.property(id)?, new.property(id)?);
                (!fields.is_empty()).then(|| TermChange {
                    id: (*id).to_owned(),
                    fields,
                })
            })
            .collect();

        // Severity per owning source: added and changed terms belong to their source in the
        // new ontology, removed terms to their source in the old one.
        let mut severity: BTreeMap<String, Compatibility> = BTreeMap::new();
        let mut raise = |source: Option<&String>, level: Compatibility| {
            if let Some(source) = source {
                let entry = severity
                    .entry(source.clone())
                    .or_insert(Compatibility::Identical);
                *entry = (*entry).max(level);
            }
        };
        for id in &added_classes {
            raise(new.class(id).map(|c| &c.source), Compatibility::Additive);
        }
        for id in &removed_classes {
            raise(old.class(id).map(|c| &c.source), Compatibility::Breaking);
        }
        for change in &changed_classes {
            let level = change
                .fields
                .iter()
                .map(class_severity)
                .max()
                .unwrap_or(Compatibility::Identical);
            raise(new.class(&change.id).map(|c| &c.source), level);
        }
        for id in &added_properties {
            if let Some(property) = new.property(id) {
                // A new required property invalidates existing statements of old classes.
                let level = if property.required
                    && property.domain.iter().any(|d| old.class(d).is_some())
                {
                    Compatibility::Breaking
                } else {
                    Compatibility::Additive
                };
                raise(Some(&property.source), level);
            }
        }
        for id in &removed_properties {
            raise(old.property(id).map(|p| &p.source), Compatibility::Breaking);
        }
        for change in &changed_properties {
            let level = change
                .fields
                .iter()
                .map(property_severity)
                .max()
                .unwrap_or(Compatibility::Identical);
            raise(new.property(&change.id).map(|p| &p.source), level);
        }

        let source_ids: BTreeSet<&String> = old
            .sources
            .iter()
            .chain(new.sources.iter())
            .map(|s| &s.id)
            .collect();
        let sources: Vec<SourceChange> = source_ids
            .into_iter()
            .map(|id| {
                let from = old.source(id).map(|s| s.version.clone());
                let to = new.source(id).map(|s| s.version.clone());
                let compatibility = if to.is_none() {
                    Compatibility::Breaking
                } else {
                    severity
                        .get(id)
                        .copied()
                        .unwrap_or(Compatibility::Identical)
                };
                SourceChange {
                    source: id.clone(),
                    from,
                    to,
                    compatibility,
                }
            })
            .collect();
        let compatibility = sources
            .iter()
            .map(|s| s.compatibility)
            .max()
            .unwrap_or(Compatibility::Identical);

        // Classes whose extraction output can change.
        let reextract_class = |f: &ClassField| class_severity(f) > Compatibility::Cosmetic;
        let reextract_property = |f: &PropertyField| property_severity(f) > Compatibility::Cosmetic;
        let mut affected: BTreeSet<String> = BTreeSet::new();
        affected.extend(removed_classes.iter().cloned());
        for change in &changed_classes {
            if change.fields.iter().any(reextract_class) {
                affected.insert(change.id.clone());
            }
        }
        let mut property_domains = |property: Option<&Property>| {
            if let Some(property) = property {
                affected.extend(property.domain.iter().cloned());
            }
        };
        for id in &removed_properties {
            property_domains(old.property(id));
        }
        for id in &added_properties {
            property_domains(new.property(id));
        }
        for change in &changed_properties {
            if change.fields.iter().any(reextract_property) {
                property_domains(old.property(&change.id));
                property_domains(new.property(&change.id));
            }
        }
        let roots: Vec<String> = affected.iter().cloned().collect();
        for root in roots {
            for ontology in [old, new] {
                affected.extend(
                    ontology
                        .descendants(&root)
                        .into_iter()
                        .map(|c| c.id.clone()),
                );
            }
        }

        Self {
            from: old.version.clone(),
            to: new.version.clone(),
            added_classes,
            removed_classes,
            changed_classes,
            added_properties,
            removed_properties,
            changed_properties,
            sources,
            compatibility,
            affected_classes: affected.into_iter().collect(),
        }
    }

    /// Whether every source's version moved enough for the changes to the terms it defines
    /// (see [`SourceChange::bump_sufficient`]). Statements record the version of the source
    /// that defines their class, so each source is versioned independently.
    pub fn version_bump_sufficient(&self) -> bool {
        self.sources.iter().all(SourceChange::bump_sufficient)
    }
}

fn owned<'a>(ids: impl Iterator<Item = &'a &'a str>) -> Vec<String> {
    ids.map(|id| (*id).to_owned()).collect()
}

fn class_fields(old: &Class, new: &Class) -> Vec<ClassField> {
    let mut fields = Vec::new();
    let mut check = |changed: bool, field| {
        if changed {
            fields.push(field);
        }
    };
    check(old.label != new.label, ClassField::Label);
    check(old.description != new.description, ClassField::Description);
    check(old.parent != new.parent, ClassField::Parent);
    check(
        old.identity_keys != new.identity_keys,
        ClassField::IdentityKeys,
    );
    check(old.cue_terms != new.cue_terms, ClassField::CueTerms);
    check(
        old.cue_patterns != new.cue_patterns,
        ClassField::CuePatterns,
    );
    check(old.dynamics != new.dynamics, ClassField::Dynamics);
    check(
        old.equivalent_to != new.equivalent_to,
        ClassField::EquivalentTo,
    );
    fields
}

fn property_fields(old: &Property, new: &Property) -> Vec<PropertyField> {
    let mut fields = Vec::new();
    let mut check = |changed: bool, field| {
        if changed {
            fields.push(field);
        }
    };
    let set = |v: &[String]| v.iter().cloned().collect::<BTreeSet<_>>();
    check(old.label != new.label, PropertyField::Label);
    check(
        old.description != new.description,
        PropertyField::Description,
    );
    check(set(&old.domain) != set(&new.domain), PropertyField::Domain);
    check(old.range != new.range, PropertyField::Range);
    check(
        old.cardinality != new.cardinality,
        PropertyField::Cardinality,
    );
    check(old.temporal != new.temporal, PropertyField::Temporal);
    check(old.required != new.required, PropertyField::Required);
    check(old.pattern != new.pattern, PropertyField::Pattern);
    check(
        old.pattern_is_cue != new.pattern_is_cue,
        PropertyField::PatternIsCue,
    );
    check(
        old.equivalent_to != new.equivalent_to,
        PropertyField::EquivalentTo,
    );
    fields
}
