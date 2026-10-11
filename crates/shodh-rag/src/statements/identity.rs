//! Identity tokens: short hashes that let the store find the statements a new statement
//! could supersede without scanning a whole class.
//!
//! A statement gets one token for its subject entity, and one per combination of values of
//! each identity key of its class (inherited keys included). Two statements that
//! [`shodh_ontology::Ontology::supersedes`] could match share at least one token, because
//! sameness is decided by exactly these subject ids and identity-key values.

use sha2::{Digest, Sha256};
use shodh_ontology::{Ontology, ValidStatement, Value};

/// Most value combinations hashed per identity key (bounds many-valued keys).
const MAX_COMBINATIONS_PER_KEY: usize = 16;

/// Identity tokens of a validated statement, sorted and unique.
pub fn identity_tokens(ontology: &Ontology, statement: &ValidStatement) -> Vec<String> {
    let mut tokens = Vec::new();
    if let Some(subject) = statement.subject() {
        tokens.push(hash(&format!("subject\u{1f}{}", subject.id)));
    }
    for key in ontology.identity_keys_of(statement.class()) {
        let columns: Vec<&[Value]> = key.iter().map(|p| statement.values(p)).collect();
        if columns.iter().any(|values| values.is_empty()) {
            continue;
        }
        for combination in combinations(&columns) {
            let mut material = String::from("key");
            for (property, value) in key.iter().zip(&combination) {
                material.push('\u{1f}');
                material.push_str(property);
                material.push('=');
                material.push_str(&value.to_string());
            }
            tokens.push(hash(&material));
        }
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

fn combinations<'a>(columns: &[&'a [Value]]) -> Vec<Vec<&'a Value>> {
    let mut out: Vec<Vec<&Value>> = vec![Vec::new()];
    for values in columns {
        let mut next = Vec::new();
        'outer: for prefix in &out {
            for value in values.iter() {
                let mut row = prefix.clone();
                row.push(value);
                next.push(row);
                if next.len() >= MAX_COMBINATIONS_PER_KEY {
                    break 'outer;
                }
            }
        }
        out = next;
    }
    out
}

fn hash(material: &str) -> String {
    let digest = Sha256::digest(material.as_bytes());
    hex::encode(&digest[..12])
}
