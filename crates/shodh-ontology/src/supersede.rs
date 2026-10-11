//! Deterministic supersede semantics between two validated statements.
//!
//! Rules (no LLM involved):
//! - Statements are about the same subject when both carry the same subject entity id, or
//!   when, for some identity key of the class, every key property has a shared value.
//! - `Many` properties accumulate: incoming values not already present are added.
//! - `One` + `temporal` properties supersede: the newer value becomes current and the older
//!   one is closed with `valid_to` (history is kept, never deleted). When the incoming
//!   statement is older than the existing one, the incoming value is recorded as history.
//!   Ties on time resolve in favour of the incoming statement.
//! - `One` non-temporal properties never supersede silently: a different value is a conflict.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::model::{Cardinality, Ontology};
use crate::statement::ValidStatement;
use crate::value::Value;

/// How the two statements were found to describe the same subject.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubjectMatch {
    /// Both statements reference the same subject entity.
    Subject {
        /// The shared entity id.
        id: String,
    },
    /// All properties of an identity key share a value.
    IdentityKey {
        /// The key's properties.
        properties: Vec<String>,
    },
}

/// Why two statements are treated as independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndependenceReason {
    /// The classes are unrelated (neither is a subclass of the other).
    UnrelatedClasses,
    /// The statements reference different subject entities, or an identity key differs.
    DifferentSubjects,
    /// There is no subject and no identity key present on both, so sameness is unknown.
    UnknownIdentity,
}

/// What happens to one property when the incoming statement is applied.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PropertyChange {
    /// The incoming value equals the existing one (a reinforcement, not a change).
    Unchanged {
        /// Property id.
        property: String,
    },
    /// New values are added (existing had none, or a `Many` property gains values).
    Added {
        /// Property id.
        property: String,
        /// The values to add.
        values: Vec<Value>,
    },
    /// Temporal functional property: the existing value is closed at `valid_to` and the
    /// incoming value becomes current.
    Superseded {
        /// Property id.
        property: String,
        /// The value being closed.
        previous: Value,
        /// The new current value.
        current: Value,
        /// End of validity of `previous` (the incoming statement's effective time).
        valid_to: DateTime<Utc>,
    },
    /// Temporal functional property: the incoming statement is older than the existing one,
    /// so its value is stored as history ending at `valid_to`; the existing value stays current.
    Historical {
        /// Property id.
        property: String,
        /// The historical value.
        value: Value,
        /// End of validity of `value` (the existing statement's effective time).
        valid_to: DateTime<Utc>,
    },
    /// Non-temporal functional property with a different value: needs resolution.
    Conflict {
        /// Property id.
        property: String,
        /// Existing value.
        existing: Value,
        /// Incoming value.
        incoming: Value,
    },
}

/// Result of [`Ontology::supersedes`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum SupersedeDecision {
    /// Store both statements; neither affects the other.
    Independent {
        /// Why.
        reason: IndependenceReason,
    },
    /// Same subject: apply the per-property changes (declaration order).
    Merge {
        /// How sameness was established.
        matched_by: SubjectMatch,
        /// Per-property outcome for every property the incoming statement carries.
        changes: Vec<PropertyChange>,
    },
}

impl SupersedeDecision {
    /// Properties whose existing value is superseded.
    pub fn superseded(&self) -> Vec<&PropertyChange> {
        self.changes()
            .iter()
            .filter(|c| matches!(c, PropertyChange::Superseded { .. }))
            .collect()
    }

    /// Conflicting properties.
    pub fn conflicts(&self) -> Vec<&PropertyChange> {
        self.changes()
            .iter()
            .filter(|c| matches!(c, PropertyChange::Conflict { .. }))
            .collect()
    }

    /// All property changes (empty for independent statements).
    pub fn changes(&self) -> &[PropertyChange] {
        match self {
            SupersedeDecision::Independent { .. } => &[],
            SupersedeDecision::Merge { changes, .. } => changes,
        }
    }
}

impl Ontology {
    /// Decides how `incoming` relates to `existing`. Both must have been validated by this
    /// ontology. The result is a pure function of the two statements.
    pub fn supersedes(
        &self,
        existing: &ValidStatement,
        incoming: &ValidStatement,
    ) -> SupersedeDecision {
        let general = if self.is_subclass_of(incoming.class(), existing.class()) {
            existing.class()
        } else if self.is_subclass_of(existing.class(), incoming.class()) {
            incoming.class()
        } else {
            return SupersedeDecision::Independent {
                reason: IndependenceReason::UnrelatedClasses,
            };
        };

        let matched_by = match self.same_subject(general, existing, incoming) {
            Ok(matched) => matched,
            Err(reason) => return SupersedeDecision::Independent { reason },
        };

        let mut changes = Vec::new();
        for property in self.properties_of(incoming.class()) {
            let new = incoming.values(&property.id);
            if new.is_empty() {
                continue;
            }
            let old = existing.values(&property.id);
            let id = property.id.clone();
            let change = match (property.cardinality, old.first(), new.first()) {
                (_, None, _) => PropertyChange::Added {
                    property: id,
                    values: new.to_vec(),
                },
                (Cardinality::Many, Some(_), _) => {
                    let added: Vec<Value> =
                        new.iter().filter(|v| !old.contains(v)).cloned().collect();
                    if added.is_empty() {
                        PropertyChange::Unchanged { property: id }
                    } else {
                        PropertyChange::Added {
                            property: id,
                            values: added,
                        }
                    }
                }
                (Cardinality::One, Some(previous), Some(current)) => {
                    if previous == current {
                        PropertyChange::Unchanged { property: id }
                    } else if !property.temporal {
                        PropertyChange::Conflict {
                            property: id,
                            existing: previous.clone(),
                            incoming: current.clone(),
                        }
                    } else if incoming.effective_from() >= existing.effective_from() {
                        PropertyChange::Superseded {
                            property: id,
                            previous: previous.clone(),
                            current: current.clone(),
                            valid_to: incoming.effective_from(),
                        }
                    } else {
                        PropertyChange::Historical {
                            property: id,
                            value: current.clone(),
                            valid_to: existing.effective_from(),
                        }
                    }
                }
                (Cardinality::One, Some(_), None) => continue,
            };
            changes.push(change);
        }
        SupersedeDecision::Merge {
            matched_by,
            changes,
        }
    }

    fn same_subject(
        &self,
        class: &str,
        existing: &ValidStatement,
        incoming: &ValidStatement,
    ) -> Result<SubjectMatch, IndependenceReason> {
        if let (Some(a), Some(b)) = (existing.subject(), incoming.subject()) {
            return if a.id == b.id {
                Ok(SubjectMatch::Subject { id: a.id.clone() })
            } else {
                Err(IndependenceReason::DifferentSubjects)
            };
        }
        let mut contradicted = false;
        for key in self.identity_keys_of(class) {
            let mut all_present = true;
            let mut all_shared = true;
            for property in key {
                let a = existing.values(property);
                let b = incoming.values(property);
                if a.is_empty() || b.is_empty() {
                    all_present = false;
                    break;
                }
                if !a.iter().any(|v| b.contains(v)) {
                    all_shared = false;
                }
            }
            if !all_present {
                continue;
            }
            if all_shared {
                return Ok(SubjectMatch::IdentityKey {
                    properties: key.to_vec(),
                });
            }
            contradicted = true;
        }
        Err(if contradicted {
            IndependenceReason::DifferentSubjects
        } else {
            IndependenceReason::UnknownIdentity
        })
    }
}
