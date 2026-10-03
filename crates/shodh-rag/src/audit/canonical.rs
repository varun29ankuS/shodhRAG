//! Canonical JSON: object keys sorted (byte order), no insignificant
//! whitespace, numbers and strings as `serde_json` prints them.
//!
//! The hash chain must not depend on `serde_json`'s map ordering, which
//! changes when any crate in the build enables its `preserve_order` feature.

use serde_json::Value;

/// Serialise `value` canonically.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_string(s: &str, out: &mut String) {
    // Serialising a `&str` cannot fail; the fallback keeps the function total.
    match serde_json::to_string(s) {
        Ok(quoted) => out.push_str(&quoted),
        Err(_) => out.push_str("\"\""),
    }
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                if let Some(v) = map.get(key) {
                    write_value(v, out);
                }
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_are_sorted_recursively_and_whitespace_free() {
        let value = json!({"b": 1, "a": {"z": [true, null], "y": "q\"x"}, "c": 1.5});
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"y":"q\"x","z":[true,null]},"b":1,"c":1.5}"#
        );
    }

    #[test]
    fn insertion_order_does_not_matter() {
        let mut first = serde_json::Map::new();
        first.insert("x".into(), json!(1));
        first.insert("a".into(), json!(2));
        let mut second = serde_json::Map::new();
        second.insert("a".into(), json!(2));
        second.insert("x".into(), json!(1));
        assert_eq!(
            canonical_json(&Value::Object(first)),
            canonical_json(&Value::Object(second))
        );
    }
}
