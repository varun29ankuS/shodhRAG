//! Version compatibility between two compiled ontologies.

use std::collections::BTreeSet;

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
    /// Overall compatibility.
    pub compatibility: Compatibility,
    /// Classes whose documents must be re-extracted (a new generation), sorted.
    /// Includes subclasses of affected classes. Dynamics, labels, descriptions and
    /// equivalences do not require re-extraction.
    pub affected_classes: Vec<String>,
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

        let breaking_class = |f: &ClassField| matches!(f, ClassField::Parent);
        let additive_class = |f: &ClassField| {
            matches!(
                f,
                ClassField::IdentityKeys | ClassField::CueTerms | ClassField::CuePatterns
            )
        };
        let breaking_property = |f: &PropertyField| {
            matches!(
                f,
                PropertyField::Domain
                    | PropertyField::Range
                    | PropertyField::Cardinality
                    | PropertyField::Temporal
                    | PropertyField::Required
                    | PropertyField::Pattern
            )
        };
        let added_required = added_properties
            .iter()
            .filter_map(|id| new.property(id))
            .any(|p| p.required && p.domain.iter().any(|d| old.class(d).is_some()));

        let breaking = !removed_classes.is_empty()
            || !removed_properties.is_empty()
            || added_required
            || changed_classes
                .iter()
                .any(|c| c.fields.iter().any(breaking_class))
            || changed_properties
                .iter()
                .any(|p| p.fields.iter().any(breaking_property));
        let additive_property = |f: &PropertyField| matches!(f, PropertyField::PatternIsCue);
        let additive = !added_classes.is_empty()
            || !added_properties.is_empty()
            || changed_classes
                .iter()
                .any(|c| c.fields.iter().any(additive_class))
            || changed_properties
                .iter()
                .any(|p| p.fields.iter().any(additive_property));
        let compatibility = if breaking {
            Compatibility::Breaking
        } else if additive {
            Compatibility::Additive
        } else if !changed_classes.is_empty() || !changed_properties.is_empty() {
            Compatibility::Cosmetic
        } else {
            Compatibility::Identical
        };

        // Classes whose extraction output can change.
        let mut affected: BTreeSet<String> = BTreeSet::new();
        affected.extend(removed_classes.iter().cloned());
        for change in &changed_classes {
            if change
                .fields
                .iter()
                .any(|f| breaking_class(f) || additive_class(f))
            {
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
            if change
                .fields
                .iter()
                .any(|f| breaking_property(f) || additive_property(f))
            {
                property_domains(old.property(&change.id));
                property_domains(new.property(&change.id));
            }
        }
        let roots: Vec<String> = affected.iter().cloned().collect();
        for root in roots {
            for ontology in [old, new] {
                affected.extend(ontology.descendants(&root).into_iter().map(|c| c.id.clone()));
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
            compatibility,
            affected_classes: affected.into_iter().collect(),
        }
    }

    /// Whether the new version number is large enough for the changes: any change needs a
    /// higher version, additive changes a minor bump, breaking changes a major bump
    /// (a minor bump for `0.x`, following semver caret rules).
    pub fn version_bump_sufficient(&self) -> bool {
        let (from, to) = (&self.from, &self.to);
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
    check(old.identity_keys != new.identity_keys, ClassField::IdentityKeys);
    check(old.cue_terms != new.cue_terms, ClassField::CueTerms);
    check(old.cue_patterns != new.cue_patterns, ClassField::CuePatterns);
    check(old.dynamics != new.dynamics, ClassField::Dynamics);
    check(old.equivalent_to != new.equivalent_to, ClassField::EquivalentTo);
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
    check(old.description != new.description, PropertyField::Description);
    check(set(&old.domain) != set(&new.domain), PropertyField::Domain);
    check(old.range != new.range, PropertyField::Range);
    check(old.cardinality != new.cardinality, PropertyField::Cardinality);
    check(old.temporal != new.temporal, PropertyField::Temporal);
    check(old.required != new.required, PropertyField::Required);
    check(old.pattern != new.pattern, PropertyField::Pattern);
    check(old.pattern_is_cue != new.pattern_is_cue, PropertyField::PatternIsCue);
    check(old.equivalent_to != new.equivalent_to, PropertyField::EquivalentTo);
    fields
}
