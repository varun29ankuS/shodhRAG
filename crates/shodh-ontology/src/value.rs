//! Property values: raw (as produced by extractors) and typed (after validation).

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, FixedOffset, NaiveDate};
use serde::{Deserialize, Serialize};

/// A reference to an entity, by stable id, optionally stating its class.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRef {
    /// Stable entity id assigned by the entity store.
    pub id: String,
    /// Class of the referenced entity, when known. Checked against the property range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

impl EntityRef {
    /// A reference without a class.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            class: None,
        }
    }

    /// A reference that states the entity's class.
    pub fn typed(id: impl Into<String>, class: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            class: Some(class.into()),
        }
    }
}

/// A monetary amount as written by an extractor, before validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMoney {
    /// Decimal amount as text (`"1250.00"`). Floats are not accepted for money.
    pub amount: String,
    /// ISO 4217 currency code (`"INR"`).
    pub currency: String,
}

/// A property value as produced by an extractor. Text is a lexical form that is parsed
/// according to the property's range; values are never coerced into a different meaning
/// (for example `"1,250"` is not a valid decimal).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RawValue {
    /// Boolean literal.
    Boolean(bool),
    /// Integer literal.
    Integer(i64),
    /// Floating-point literal (accepted for `Decimal` ranges only, never for `Money`).
    Float(f64),
    /// Lexical form of any datatype.
    Text(String),
    /// Money with decimal amount and currency.
    Money(RawMoney),
    /// Reference to an entity (object properties).
    Entity(EntityRef),
    /// Several values for a `Many` property.
    List(Vec<RawValue>),
}

impl RawValue {
    /// Convenience constructor for text.
    pub fn text(value: impl Into<String>) -> Self {
        RawValue::Text(value.into())
    }

    /// Convenience constructor for money.
    pub fn money(amount: impl Into<String>, currency: impl Into<String>) -> Self {
        RawValue::Money(RawMoney {
            amount: amount.into(),
            currency: currency.into(),
        })
    }

    /// Convenience constructor for an entity reference.
    pub fn entity(id: impl Into<String>) -> Self {
        RawValue::Entity(EntityRef::new(id))
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            RawValue::Boolean(_) => "boolean",
            RawValue::Integer(_) => "integer",
            RawValue::Float(_) => "float",
            RawValue::Text(_) => "text",
            RawValue::Money(_) => "money",
            RawValue::Entity(_) => "entity reference",
            RawValue::List(_) => "list",
        }
    }
}

impl From<&str> for RawValue {
    fn from(value: &str) -> Self {
        RawValue::Text(value.to_owned())
    }
}

impl From<String> for RawValue {
    fn from(value: String) -> Self {
        RawValue::Text(value)
    }
}

impl From<i64> for RawValue {
    fn from(value: i64) -> Self {
        RawValue::Integer(value)
    }
}

impl From<bool> for RawValue {
    fn from(value: bool) -> Self {
        RawValue::Boolean(value)
    }
}

impl From<EntityRef> for RawValue {
    fn from(value: EntityRef) -> Self {
        RawValue::Entity(value)
    }
}

/// Why a lexical form is not a valid decimal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a plain decimal (expected -?digits[.digits], no separators or exponent)")]
pub struct DecimalError(pub String);

/// An exact decimal number in canonical form: no leading zeros, no trailing fractional
/// zeros, no `-0`. Equality is numeric equality.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Decimal(String);

impl Decimal {
    /// Canonical text form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Decimal {
    type Err = DecimalError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = || DecimalError(text.to_owned());
        let (negative, body) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (integer, fraction) = match body.split_once('.') {
            Some((integer, fraction)) => (integer, Some(fraction)),
            None => (body, None),
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(integer) || fraction.is_some_and(|f| !digits(f)) {
            return Err(error());
        }
        let integer = integer.trim_start_matches('0');
        let integer = if integer.is_empty() { "0" } else { integer };
        let fraction = fraction.map(|f| f.trim_end_matches('0')).unwrap_or("");
        let is_zero = integer == "0" && fraction.is_empty();
        let mut canonical = String::with_capacity(text.len());
        if negative && !is_zero {
            canonical.push('-');
        }
        canonical.push_str(integer);
        if !fraction.is_empty() {
            canonical.push('.');
            canonical.push_str(fraction);
        }
        Ok(Decimal(canonical))
    }
}

/// An exact monetary amount.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Money {
    /// Amount.
    pub amount: Decimal,
    /// ISO 4217 alphabetic currency code (three upper-case ASCII letters).
    pub currency: String,
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.amount, self.currency)
    }
}

/// A validated, typed property value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value")]
pub enum Value {
    /// Free text.
    Text(String),
    /// Boolean.
    Boolean(bool),
    /// Integer.
    Integer(i64),
    /// Exact decimal.
    Decimal(Decimal),
    /// Money.
    Money(Money),
    /// Calendar date.
    Date(NaiveDate),
    /// Instant with offset.
    DateTime(DateTime<FixedOffset>),
    /// Absolute URL.
    Url(String),
    /// E-mail address.
    Email(String),
    /// One of the property's enum values.
    Enum(String),
    /// Reference to an entity.
    Entity(EntityRef),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Text(v) | Value::Url(v) | Value::Email(v) | Value::Enum(v) => f.write_str(v),
            Value::Boolean(v) => write!(f, "{v}"),
            Value::Integer(v) => write!(f, "{v}"),
            Value::Decimal(v) => write!(f, "{v}"),
            Value::Money(v) => write!(f, "{v}"),
            Value::Date(v) => write!(f, "{}", v.format("%Y-%m-%d")),
            Value::DateTime(v) => f.write_str(&v.to_rfc3339()),
            Value::Entity(v) => write!(f, "@{}", v.id),
        }
    }
}

/// Parses an ISO 8601 calendar date in exactly the `YYYY-MM-DD` form.
pub(crate) fn parse_date(text: &str) -> Option<NaiveDate> {
    let bytes = text.as_bytes();
    let shape_ok = bytes.len() == 10
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if !shape_ok {
        return None;
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// Parses an ISO 8601 / RFC 3339 date-time. An explicit UTC offset (`Z` or `+05:30`) is
/// required: a local time without an offset is ambiguous and is rejected.
pub(crate) fn parse_datetime(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(text).ok()
}

/// Whether `code` has the shape of an ISO 4217 alphabetic code.
pub(crate) fn is_currency_code(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())
}

/// Whether `text` is an absolute URL: `scheme://host[...]` with no whitespace.
pub(crate) fn is_url(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once("://") else {
        return false;
    };
    let scheme_ok = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    scheme_ok
        && !host.is_empty()
        && !text
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>\"{}|\\^`".contains(c))
}

/// Whether `text` looks like an e-mail address (`local@domain.tld`, no whitespace).
pub(crate) fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    let clean = |s: &str| !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c == '@');
    clean(local)
        && clean(domain)
        && domain.split('.').all(|label| !label.is_empty())
        && domain.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_canonical_form() {
        let canonical = |s: &str| s.parse::<Decimal>().map(|d| d.to_string());
        assert_eq!(canonical("0012.500"), Ok("12.5".to_owned()));
        assert_eq!(canonical("-0.00"), Ok("0".to_owned()));
        assert_eq!(canonical("7"), Ok("7".to_owned()));
        assert!(canonical("1,250.00").is_err());
        assert!(canonical("1e3").is_err());
        assert!(canonical("12.").is_err());
        assert!(canonical(".5").is_err());
        assert!(canonical("+5").is_err());
        assert!(canonical("").is_err());
    }

    #[test]
    fn strict_dates() {
        assert!(parse_date("2026-10-03").is_some());
        assert!(parse_date("2024-02-29").is_some());
        assert!(parse_date("2024-1-5").is_none());
        assert!(parse_date("2024-02-30").is_none());
        assert!(parse_date("2023-02-29").is_none());
        assert!(parse_date("03/10/2026").is_none());
        assert!(parse_datetime("2026-10-03T10:00:00+05:30").is_some());
        assert!(parse_datetime("2026-10-03T10:00:00Z").is_some());
        assert!(parse_datetime("2026-10-03T10:00:00").is_none());
    }

    #[test]
    fn url_email_currency() {
        assert!(is_url("https://example.com/a?b#c"));
        assert!(!is_url("example.com"));
        assert!(!is_url("https:// bad"));
        assert!(is_email("a.b@example.co.in"));
        assert!(!is_email("a@b"));
        assert!(!is_email("a b@example.com"));
        assert!(is_currency_code("INR"));
        assert!(!is_currency_code("inr"));
        assert!(!is_currency_code("RUPEE"));
    }
}
