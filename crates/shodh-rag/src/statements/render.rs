//! Plain-text rendering of a statement, used for search (embedding and full text) and for
//! display. Deterministic: properties in the ontology's declaration order.

use shodh_ontology::{Ontology, ValidStatement, Value};

/// The property that carries a note's free text.
const NOTE_TEXT: &str = "noteText";

/// The research pack's snippet class, rendered as its title, text and note (what a reader
/// searches for), not its page and rectangle.
const SNIPPET: &str = "Snippet";
const SNIPPET_TEXT: [&str; 3] = ["snippetTitle", "snippetText", "snippetNote"];

/// Canonical entity id of the app's user (a `Person`). Every statement about the user
/// references this id, so identity keys such as a preference's holder compare equal.
pub const SELF_ENTITY_ID: &str = "person:self";

fn value_text(value: &Value) -> String {
    match value {
        Value::Entity(entity) if entity.id == SELF_ENTITY_ID => "the user".to_string(),
        other => other.to_string(),
    }
}

/// Renders a validated statement as one line of text.
///
/// A `Note` renders as its text, a research `Snippet` as its title, text and note. Any other
/// class renders as
/// `Label: property value; property value, value`, with entity references as `@id` (the
/// user's own entity as "the user").
pub fn render_text(ontology: &Ontology, statement: &ValidStatement) -> String {
    if let [Value::Text(text)] = statement.values(NOTE_TEXT) {
        if ontology.is_subclass_of(statement.class(), "Note") {
            return text.trim().to_string();
        }
    }
    if ontology.is_subclass_of(statement.class(), SNIPPET) {
        let parts: Vec<&str> = SNIPPET_TEXT
            .iter()
            .filter_map(|p| match statement.values(p) {
                [Value::Text(text)] => Some(text.trim()),
                _ => None,
            })
            .filter(|t| !t.is_empty())
            .collect();
        if !parts.is_empty() {
            return parts.join("\n");
        }
    }
    let label = ontology
        .class(statement.class())
        .map(|c| c.label.as_str())
        .unwrap_or(statement.class());
    let mut parts = Vec::new();
    for property in ontology.properties_of(statement.class()) {
        let values = statement.values(&property.id);
        if values.is_empty() {
            continue;
        }
        let rendered: Vec<String> = values.iter().map(value_text).collect();
        parts.push(format!("{} {}", property.label, rendered.join(", ")));
    }
    let mut text = String::from(label);
    if let Some(subject) = statement.subject() {
        if subject.id == SELF_ENTITY_ID {
            text.push_str(" (the user)");
        } else {
            text.push_str(&format!(" @{}", subject.id));
        }
    }
    if !parts.is_empty() {
        text.push_str(": ");
        text.push_str(&parts.join("; "));
    }
    text
}

/// The values of a statement as plain words, for full-text search: no class or property
/// labels (every preference would otherwise match "preference"), and no reference to the
/// user (every fact about the user would match "user"). A note is its text.
pub fn render_terms(ontology: &Ontology, statement: &ValidStatement) -> String {
    let mut terms = Vec::new();
    if let Some(subject) = statement.subject() {
        if subject.id != SELF_ENTITY_ID {
            terms.push(subject.id.clone());
        }
    }
    for property in ontology.properties_of(statement.class()) {
        for value in statement.values(&property.id) {
            match value {
                Value::Entity(entity) if entity.id == SELF_ENTITY_ID => {}
                Value::Entity(entity) => terms.push(entity.id.clone()),
                other => terms.push(other.to_string()),
            }
        }
    }
    terms.join(" ")
}
