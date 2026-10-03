//! Plain-text rendering of a statement, used for search (embedding and full text) and for
//! display. Deterministic: properties in the ontology's declaration order.

use shodh_ontology::{Ontology, ValidStatement, Value};

/// The property that carries a note's free text.
const NOTE_TEXT: &str = "noteText";

/// Renders a validated statement as one line of text.
///
/// A `Note` renders as its text. Any other class renders as
/// `Label: property value; property value, value`, with entity references as `@id`.
pub fn render_text(ontology: &Ontology, statement: &ValidStatement) -> String {
    if let [Value::Text(text)] = statement.values(NOTE_TEXT) {
        if ontology.is_subclass_of(statement.class(), "Note") {
            return text.trim().to_string();
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
        let rendered: Vec<String> = values.iter().map(ToString::to_string).collect();
        parts.push(format!("{} {}", property.label, rendered.join(", ")));
    }
    let mut text = String::from(label);
    if let Some(subject) = statement.subject() {
        text.push_str(&format!(" @{}", subject.id));
    }
    if !parts.is_empty() {
        text.push_str(": ");
        text.push_str(&parts.join("; "));
    }
    text
}
