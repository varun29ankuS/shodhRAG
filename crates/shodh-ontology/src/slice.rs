//! Ontology slicing: select the classes relevant to a piece of text so that a constrained
//! extraction step (GLiNER2 labels, grounded LLM) sees only a small, relevant schema.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::Serialize;

use crate::model::{Cardinality, Class, Datatype, Ontology, Property, Range};

/// Why a class is part of a slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceRole {
    /// Its cue terms, cue patterns or a property pattern matched the text.
    Matched,
    /// Ancestor of a matched class (contributes inherited properties).
    Ancestor,
    /// Range of a relation of a matched class (may be referenced by id).
    Referenced,
}

/// Evidence that a class matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SliceMatch {
    /// The matched class.
    pub class: String,
    /// The text that matched (first occurrence).
    pub evidence: String,
    /// What matched: `cue` or `pattern:<property>`.
    pub via: String,
}

/// One class in a slice.
#[derive(Debug, Clone, Serialize)]
pub struct SliceClass<'a> {
    /// The class.
    pub class: &'a Class,
    /// Its role.
    pub role: SliceRole,
}

/// The relevant part of the ontology for a text.
#[derive(Debug, Clone, Serialize)]
pub struct OntologySlice<'a> {
    /// Ontology id.
    pub ontology_id: &'a str,
    /// Ontology version.
    pub ontology_version: String,
    /// Match evidence, in class load order.
    pub matches: Vec<SliceMatch>,
    /// Classes in the slice, in load order.
    pub classes: Vec<SliceClass<'a>>,
    /// Properties applicable to matched classes (own and inherited), in load order.
    pub properties: Vec<&'a Property>,
}

impl OntologySlice<'_> {
    /// Whether nothing matched.
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Ids of matched classes.
    pub fn matched_classes(&self) -> Vec<&str> {
        self.classes
            .iter()
            .filter(|c| c.role == SliceRole::Matched)
            .map(|c| c.class.id.as_str())
            .collect()
    }

    /// Whether the class is in the slice in any role.
    pub fn contains(&self, class: &str) -> bool {
        self.classes.iter().any(|c| c.class.id == class)
    }

    /// Compact, deterministic prompt text describing the slice for a constrained
    /// extraction step. Each matched class lists every applicable property with its range,
    /// cardinality and constraints.
    pub fn render_prompt(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Ontology {} {}. Extract only these classes and properties; never invent others. \
Relations take an entity id. Dates are YYYY-MM-DD; Money is \"<decimal> <ISO 4217>\".",
            self.ontology_id, self.ontology_version
        );
        for entry in self.classes.iter().filter(|c| c.role == SliceRole::Matched) {
            let class = entry.class;
            let parent = class
                .parent
                .as_deref()
                .map(|p| format!(" (is-a {p})"))
                .unwrap_or_default();
            let _ = writeln!(out, "\n{}{}: {}", class.id, parent, class.description);
            for property in self
                .properties
                .iter()
                .filter(|p| p.domain.iter().any(|d| self.is_ancestor_or_self(class, d)))
            {
                let _ = writeln!(out, "  - {}", render_property(property));
            }
        }
        let referenced: Vec<&str> = self
            .classes
            .iter()
            .filter(|c| c.role == SliceRole::Referenced)
            .map(|c| c.class.id.as_str())
            .collect();
        if !referenced.is_empty() {
            let _ = writeln!(out, "\nReferenced classes: {}", referenced.join(", "));
        }
        out
    }

    fn is_ancestor_or_self(&self, class: &Class, candidate: &str) -> bool {
        if class.id == candidate {
            return true;
        }
        // Walk the parents present in the slice (ancestors are always included).
        let mut current = class.parent.as_deref();
        while let Some(parent) = current {
            if parent == candidate {
                return true;
            }
            current = self
                .classes
                .iter()
                .find(|c| c.class.id == parent)
                .and_then(|c| c.class.parent.as_deref());
        }
        false
    }
}

fn render_property(property: &Property) -> String {
    let range = match &property.range {
        Range::Class(class) => format!("-> {class}"),
        Range::Datatype(Datatype::Enum(values)) => format!("one of [{}]", values.join(", ")),
        Range::Datatype(datatype) => datatype.name().to_owned(),
    };
    let mut flags = vec![match property.cardinality {
        Cardinality::One => "one",
        Cardinality::Many => "many",
    }];
    if property.required {
        flags.push("required");
    }
    if property.temporal {
        flags.push("changes over time");
    }
    let pattern = property
        .pattern
        .as_deref()
        .map(|p| format!(" /{p}/"))
        .unwrap_or_default();
    format!(
        "{}: {}{} [{}]",
        property.id,
        range,
        pattern,
        flags.join(", ")
    )
}

impl Ontology {
    /// Selects the classes whose cue terms, cue patterns or property value patterns match
    /// `text`, plus their ancestors and the classes their relations point to.
    pub fn slice_for(&self, text: &str) -> OntologySlice<'_> {
        let mut matches: Vec<SliceMatch> = Vec::new();
        let mut matched: BTreeSet<usize> = BTreeSet::new();

        for (index, class) in self.classes.iter().enumerate() {
            if let Some(found) = self
                .cue_matchers
                .get(index)
                .and_then(Option::as_ref)
                .and_then(|m| m.find(text))
            {
                matched.insert(index);
                matches.push(SliceMatch {
                    class: class.id.clone(),
                    evidence: found.as_str().to_owned(),
                    via: "cue".to_owned(),
                });
            }
        }
        for (index, property) in self.properties.iter().enumerate() {
            let Some(found) = self
                .scan_patterns
                .get(index)
                .and_then(Option::as_ref)
                .and_then(|m| m.find(text))
            else {
                continue;
            };
            for domain in &property.domain {
                if let Some(&class_index) = self.class_index.get(domain) {
                    if matched.insert(class_index) {
                        matches.push(SliceMatch {
                            class: domain.clone(),
                            evidence: found.as_str().to_owned(),
                            via: format!("pattern:{}", property.id),
                        });
                    }
                }
            }
        }
        matches.sort_by_key(|m| self.class_index.get(&m.class).copied());

        let mut roles: Vec<Option<SliceRole>> = vec![None; self.classes.len()];
        let mut property_set: BTreeSet<usize> = BTreeSet::new();
        for &index in &matched {
            roles[index] = Some(SliceRole::Matched);
            for &p in &self.applicable[index] {
                property_set.insert(p);
            }
        }
        for &index in &matched {
            for ancestor in self.ancestors(&self.classes[index].id).into_iter().skip(1) {
                if let Some(&a) = self.class_index.get(&ancestor.id) {
                    if roles[a].is_none() {
                        roles[a] = Some(SliceRole::Ancestor);
                    }
                }
            }
        }
        for &p in &property_set {
            if let Range::Class(target) = &self.properties[p].range {
                if let Some(&t) = self.class_index.get(target) {
                    if roles[t].is_none() {
                        roles[t] = Some(SliceRole::Referenced);
                    }
                }
            }
        }

        OntologySlice {
            ontology_id: &self.id,
            ontology_version: self.version.to_string(),
            matches,
            classes: self
                .classes
                .iter()
                .zip(roles)
                .filter_map(|(class, role)| role.map(|role| SliceClass { class, role }))
                .collect(),
            properties: property_set
                .into_iter()
                .map(|p| &self.properties[p])
                .collect(),
        }
    }
}
