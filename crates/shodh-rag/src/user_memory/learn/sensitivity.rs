//! Sensitive memories: never applied automatically, always shown to the user first.
//!
//! A statement is sensitive when it is about health, carries a financial identifier, looks
//! like a credential, or is personal data about someone other than the user. The rules are
//! deliberately broad: a false positive costs one click, a false negative stores something
//! the user never saw.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use shodh_ontology::{Ontology, ValidStatement, Value};

use crate::statements::SELF_ENTITY_ID;

/// Why a statement is sensitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveReason {
    /// Health, medical or mental-health information.
    Health,
    /// Tax ids, account, card or payment identifiers, payments and amounts.
    FinancialIdentifier,
    /// Passwords, PINs, one-time codes, API keys, tokens and other secrets.
    Credential,
    /// Personal data about someone other than the user.
    ThirdPartyPersonal,
}

impl SensitiveReason {
    /// Stable name (`health`, `financial_identifier`, `credential`,
    /// `third_party_personal`).
    pub fn label(self) -> &'static str {
        match self {
            SensitiveReason::Health => "health",
            SensitiveReason::FinancialIdentifier => "financial_identifier",
            SensitiveReason::Credential => "credential",
            SensitiveReason::ThirdPartyPersonal => "third_party_personal",
        }
    }
}

/// Classes whose statements are financial records or identifiers.
const FINANCIAL_CLASSES: [&str; 5] = ["TaxId", "Payment", "Amount", "Invoice", "LineItem"];

/// Properties that hold financial identifiers.
const FINANCIAL_PROPERTIES: [&str; 3] = ["gstin", "pan", "hasTaxId"];

/// Properties that hold personal contact data.
const PERSONAL_PROPERTIES: [&str; 2] = ["email", "hasAddress"];

fn regex(cell: &'static OnceLock<Option<Regex>>, pattern: &str) -> Option<&'static Regex> {
    cell.get_or_init(|| Regex::new(pattern).ok()).as_ref()
}

static CREDENTIAL: OnceLock<Option<Regex>> = OnceLock::new();
static HEALTH: OnceLock<Option<Regex>> = OnceLock::new();
static FINANCIAL: OnceLock<Option<Regex>> = OnceLock::new();
static CARD: OnceLock<Option<Regex>> = OnceLock::new();

const CREDENTIAL_PATTERN: &str = r"(?i)\b(pass(word|code|phrase)?s?|passwd|pwd|pin( ?code)?|otp|one[- ]time (code|password)|2fa|mfa|api[ _-]?keys?|secret|token|private[ _-]?key|ssh[ _-]?key|recovery (code|phrase)|seed phrase|cvv|cvc|login|credentials?)\b";

const HEALTH_PATTERN: &str = r"(?i)\b(health|medical|medicine|medication|medicines|prescri\w*|diagnos\w*|doctor|physician|hospital|clinic|therap\w*|disease|illness|symptoms?|surgery|allerg\w*|diabet\w*|cancer|asthma|depress\w*|anxiety|adhd|autis\w*|bipolar|pregnan\w*|hiv|blood (pressure|sugar|group|type)|cholesterol|mental|psychiatr\w*|disabilit\w*|insulin|dose|dosage)\b";

/// Account numbers, IFSC, IBAN, Aadhaar, PAN, GSTIN, UPI ids.
const FINANCIAL_PATTERN: &str = r"(?i)\b(account (number|no)|a/c|ifsc|iban|swift|routing number|aadhaar|aadhar|upi id|credit card|debit card|card number|bank account|salary|net worth|loan|tax return)\b|\b[A-Z]{4}0[A-Z0-9]{6}\b|\b[A-Z]{5}[0-9]{4}[A-Z]\b|\b[0-9]{2}[A-Z]{5}[0-9]{4}[A-Z][0-9A-Z]Z[0-9A-Z]\b|\b[0-9]{4} ?[0-9]{4} ?[0-9]{4}\b|\b[A-Z]{2}[0-9]{2}[A-Z0-9]{11,30}\b|\b[\w.-]{2,}@(ok\w+|ybl|paytm|upi|ibl|axl)\b";

/// 13 to 19 digits, optionally grouped (a payment card number when it passes Luhn).
const CARD_PATTERN: &str = r"\b(?:[0-9][ -]?){12,18}[0-9]\b";

fn luhn(digits: &str) -> bool {
    let digits: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Sensitivity of free text (also used for the evidence a suggestion quotes).
pub fn classify_text(text: &str) -> Vec<SensitiveReason> {
    let mut reasons = Vec::new();
    if regex(&CREDENTIAL, CREDENTIAL_PATTERN).is_some_and(|re| re.is_match(text)) {
        reasons.push(SensitiveReason::Credential);
    }
    if regex(&HEALTH, HEALTH_PATTERN).is_some_and(|re| re.is_match(text)) {
        reasons.push(SensitiveReason::Health);
    }
    let financial = regex(&FINANCIAL, FINANCIAL_PATTERN).is_some_and(|re| re.is_match(text))
        || regex(&CARD, CARD_PATTERN)
            .is_some_and(|re| re.find_iter(text).any(|m| luhn(m.as_str())));
    if financial {
        reasons.push(SensitiveReason::FinancialIdentifier);
    }
    reasons
}

/// Why `statement` is sensitive (empty when it is not), sorted and unique.
pub fn classify(ontology: &Ontology, statement: &ValidStatement) -> Vec<SensitiveReason> {
    let mut reasons = Vec::new();
    let class = statement.class();
    if FINANCIAL_CLASSES
        .iter()
        .any(|c| ontology.is_subclass_of(class, c))
    {
        reasons.push(SensitiveReason::FinancialIdentifier);
    }
    let about_self = statement.subject().is_some_and(|s| s.id == SELF_ENTITY_ID);
    if ontology.is_subclass_of(class, "Person") && !about_self {
        reasons.push(SensitiveReason::ThirdPartyPersonal);
    }
    for (property, values) in statement.properties() {
        if FINANCIAL_PROPERTIES.contains(&property.as_str()) {
            reasons.push(SensitiveReason::FinancialIdentifier);
        }
        if PERSONAL_PROPERTIES.contains(&property.as_str()) && !about_self {
            reasons.push(SensitiveReason::ThirdPartyPersonal);
        }
        for value in values {
            match value {
                // A reference to a person other than the user (a participant, an
                // attendee, an advisor) is personal data about them.
                Value::Entity(entity)
                    if entity.id != SELF_ENTITY_ID
                        && (entity
                            .class
                            .as_deref()
                            .is_some_and(|c| ontology.is_subclass_of(c, "Person"))
                            || entity.id.starts_with("person:")
                            || property_targets_person(ontology, property)) =>
                {
                    reasons.push(SensitiveReason::ThirdPartyPersonal);
                }
                Value::Money(_) => reasons.push(SensitiveReason::FinancialIdentifier),
                Value::Email(_) if !about_self => reasons.push(SensitiveReason::ThirdPartyPersonal),
                Value::Text(text) => reasons.extend(classify_text(text)),
                _ => {}
            }
        }
    }
    reasons.sort();
    reasons.dedup();
    reasons
}

fn property_targets_person(ontology: &Ontology, property: &str) -> bool {
    ontology.property(property).is_some_and(|p| {
        matches!(&p.range, shodh_ontology::Range::Class(range) if ontology.is_subclass_of(range, "Person"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rules() {
        let has = |text: &str, reason| classify_text(text).contains(&reason);
        assert!(has(
            "my wifi password is hunter2",
            SensitiveReason::Credential
        ));
        assert!(has("The API key is sk-123", SensitiveReason::Credential));
        assert!(has("I take insulin every morning", SensitiveReason::Health));
        assert!(has("diagnosed with asthma", SensitiveReason::Health));
        assert!(has("PAN ABCDE1234F", SensitiveReason::FinancialIdentifier));
        assert!(has(
            "IFSC HDFC0001234",
            SensitiveReason::FinancialIdentifier
        ));
        assert!(has(
            "card 4111 1111 1111 1111",
            SensitiveReason::FinancialIdentifier
        ));
        assert!(has(
            "aadhaar 1234 5678 9012",
            SensitiveReason::FinancialIdentifier
        ));
        // Not every long number is a card (Luhn fails) and ordinary text is fine.
        assert!(!has(
            "order 1234567890123",
            SensitiveReason::FinancialIdentifier
        ));
        assert!(classify_text("I prefer dark roast coffee").is_empty());
        assert!(classify_text("Project Apollo is on hold").is_empty());
    }

    #[test]
    fn luhn_checks_card_numbers() {
        assert!(luhn("4111111111111111"));
        assert!(!luhn("4111111111111112"));
        assert!(!luhn("1234"));
    }
}
