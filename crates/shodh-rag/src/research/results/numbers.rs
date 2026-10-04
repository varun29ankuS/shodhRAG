//! Reading one reported number from a table cell, exactly as printed.
//!
//! A cell yields a value only when it holds exactly one number (optionally signed, with
//! thousands separators, a percent sign, a `±` spread, a parenthesised delta such as
//! `(+1.2)`, and marker characters such as `*`, `†` or bold/underline remnants). The
//! printed lexeme is kept verbatim (`lexeme` is always a substring of the cell); the
//! decimal is derived from it only by dropping thousands separators and the percent sign
//! and writing a Unicode minus as `-`. Nothing is rounded or converted.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// One number read from a cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellNumber {
    /// The number exactly as printed (a substring of the cell text).
    pub lexeme: String,
    /// The decimal, canonical enough to parse: `-0.68`, `1234`, `95.30`.
    pub decimal: String,
    /// Unit printed in the cell (`%`, `ms`, `mJ`, `GB`, `B`, `M`, `K`, `x`), if any.
    pub unit: Option<String>,
}

/// Why a cell gave no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellIssue {
    /// Empty, a dash or a placeholder.
    Empty,
    /// No number at all.
    NotNumeric,
    /// Several numbers that are not one value with a spread or delta.
    SeveralNumbers,
}

static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[+\-−–]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[+\-−–]?\.\d+").expect("static regex")
});

/// Value, optional spread or delta, optional unit, markers. The value is group `v`, the
/// unit `u`.
static VALUE_CELL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^[*†‡§¶\s]*",
        r"(?P<v>[+\-−–]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[+\-−–]?\.\d+)",
        r"\s*(?P<u>%|ms|s|µs|us|mJ|J|GB|MB|KB|TB|[KMBT]|x|×)?",
        r"(?:\s*(?:±|\+/-|\+-)\s*\d+(?:\.\d+)?\s*%?)?",
        r"(?:\s*\(\s*[+\-−–]\s*\d+(?:\.\d+)?\s*%?\s*\))?",
        r"[*†‡§¶\s]*$",
    ))
    .expect("static regex")
});

fn is_placeholder(text: &str) -> bool {
    matches!(
        text,
        "" | "-" | "–" | "—" | "−" | "n/a" | "N/A" | "NA" | "–/–" | "x" | "✗" | "✓" | "?"
    )
}

/// Reads the one number of a cell, or says why there is none.
pub fn read_cell(text: &str) -> Result<CellNumber, CellIssue> {
    let trimmed = text.trim();
    if is_placeholder(trimmed) {
        return Err(CellIssue::Empty);
    }
    let Some(caps) = VALUE_CELL.captures(trimmed) else {
        return Err(if NUMBER.find_iter(trimmed).count() > 1 {
            CellIssue::SeveralNumbers
        } else {
            CellIssue::NotNumeric
        });
    };
    let Some(value) = caps.name("v") else {
        return Err(CellIssue::NotNumeric);
    };
    let lexeme = value.as_str().to_string();
    let decimal = lexeme
        .replace(',', "")
        .replace(['−', '–'], "-")
        .trim_start_matches('+')
        .to_string();
    let decimal = if let Some(rest) = decimal.strip_prefix("-.") {
        format!("-0.{rest}")
    } else if let Some(rest) = decimal.strip_prefix('.') {
        format!("0.{rest}")
    } else {
        decimal
    };
    let unit = caps.name("u").map(|u| match u.as_str() {
        "×" => "x".to_string(),
        "us" => "µs".to_string(),
        other => other.to_string(),
    });
    Ok(CellNumber {
        lexeme,
        decimal,
        unit,
    })
}

/// Whether a cell contains any digit (a candidate value cell).
pub fn has_number(text: &str) -> bool {
    NUMBER.is_match(text)
}

/// A unit stated in a header: `(%)`, `[ms]`, `(mJ)`, `in %`.
pub fn header_unit(header: &str) -> Option<String> {
    static UNIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:[(\[]\s*(%|ms|s|µs|mJ|J|GB|MB|KB|TB|x|×)\s*[)\]]|\bin\s+(%))")
            .expect("static regex")
    });
    let caps = UNIT.captures(header)?;
    caps.get(1).or_else(|| caps.get(2)).map(|m| {
        if m.as_str() == "×" {
            "x".to_string()
        } else {
            m.as_str().to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> (String, String, Option<String>) {
        let n = read_cell(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert!(text.contains(&n.lexeme), "{text} must contain {}", n.lexeme);
        (n.lexeme, n.decimal, n.unit)
    }

    #[test]
    fn single_values_are_read_verbatim() {
        assert_eq!(value("95.30"), ("95.30".into(), "95.30".into(), None));
        assert_eq!(value("−0.68"), ("−0.68".into(), "-0.68".into(), None));
        assert_eq!(value("+0.04"), ("+0.04".into(), "0.04".into(), None));
        assert_eq!(
            value("2,883,584"),
            ("2,883,584".into(), "2883584".into(), None)
        );
        assert_eq!(
            value("78.0%"),
            ("78.0".into(), "78.0".into(), Some("%".into()))
        );
        assert_eq!(
            value("445.47 mJ"),
            ("445.47".into(), "445.47".into(), Some("mJ".into()))
        );
        assert_eq!(value("95.3±0.2"), ("95.3".into(), "95.3".into(), None));
        assert_eq!(
            value("0.214 ± 0.027"),
            ("0.214".into(), "0.214".into(), None)
        );
        assert_eq!(value("95.3 (+1.2)"), ("95.3".into(), "95.3".into(), None));
        assert_eq!(value("**88.1**"), ("88.1".into(), "88.1".into(), None));
        assert_eq!(value("61.55*"), ("61.55".into(), "61.55".into(), None));
        assert_eq!(
            value("3.80B"),
            ("3.80".into(), "3.80".into(), Some("B".into()))
        );
        assert_eq!(value(".5"), (".5".into(), "0.5".into(), None));
    }

    #[test]
    fn cells_without_exactly_one_value_are_refused() {
        assert_eq!(read_cell(""), Err(CellIssue::Empty));
        assert_eq!(read_cell(" - "), Err(CellIssue::Empty));
        assert_eq!(read_cell("Mamba"), Err(CellIssue::NotNumeric));
        assert_eq!(read_cell("28.39 42.69"), Err(CellIssue::SeveralNumbers));
        assert_eq!(read_cell("DeltaNet 89.8"), Err(CellIssue::NotNumeric));
        assert_eq!(read_cell("3 × 105"), Err(CellIssue::SeveralNumbers));
        assert_eq!(read_cell("(Pass@1) 84.0"), Err(CellIssue::SeveralNumbers));
        assert!(has_number("acc n ↑ 3"));
        assert!(!has_number("acc ↑"));
    }

    #[test]
    fn header_units() {
        assert_eq!(header_unit("Accuracy (%)").as_deref(), Some("%"));
        assert_eq!(header_unit("Latency [ms]").as_deref(), Some("ms"));
        assert_eq!(header_unit("Error in %").as_deref(), Some("%"));
        assert_eq!(header_unit("Recall@10"), None);
    }
}
