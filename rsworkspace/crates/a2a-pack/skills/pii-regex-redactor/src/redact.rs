use regex::Regex;
use serde_json::Value;

const MASK: &str = "[REDACTED]";

pub fn redact_json_bytes(input: &[u8]) -> Result<Vec<u8>, RedactError> {
    let mut value: Value = serde_json::from_slice(input).map_err(RedactError::Json)?;
    redact_value(&mut value);
    serde_json::to_vec(&value).map_err(RedactError::Json)
}

pub fn redact_text(input: &str) -> String {
    let mut out = input.to_owned();
    out = mask_emails(&out);
    out = mask_ssns(&out);
    out = mask_us_phones(&out);
    mask_credit_cards(&out)
}

fn redact_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = redact_text(text);
        }
        Value::Array(items) => {
            for item in items {
                redact_value(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                redact_value(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn mask_emails(text: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b").expect("email regex")
    });
    re.replace_all(text, MASK).into_owned()
}

fn mask_ssns(text: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").expect("ssn regex"));
    re.replace_all(text, MASK).into_owned()
}

fn mask_us_phones(text: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?x)
            \b
            (?:
                (?:\+1[\s.-]?)?(?:\(\d{3}\)|\d{3})[\s.-]?\d{3}[\s.-]?\d{4}
                |
                \d{3}[\s.-]\d{3}[\s.-]\d{4}
            )
            \b
        ")
        .expect("phone regex")
    });
    re.replace_all(text, MASK).into_owned()
}

fn mask_credit_cards(text: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?x)\b(?:\d[\s-]?){13,19}\b").expect("card candidate regex")
    });
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for mat in re.find_iter(text) {
        let candidate = mat.as_str();
        let digits: String = candidate.chars().filter(char::is_ascii_digit).collect();
        if (13..=19).contains(&digits.len()) && luhn_valid(&digits) {
            out.push_str(&text[last..mat.start()]);
            out.push_str(MASK);
            last = mat.end();
        }
    }
    out.push_str(&text[last..]);
    out
}

fn luhn_valid(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut alt = false;
    for ch in digits.chars().rev() {
        let Some(mut digit) = ch.to_digit(10) else {
            return false;
        };
        if alt {
            digit *= 2;
            if digit > 9 {
                digit -= 9;
            }
        }
        sum += digit;
        alt = !alt;
    }
    sum.is_multiple_of(10)
}

#[derive(Debug)]
pub enum RedactError {
    Json(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_email_ssn_phone_and_card_in_text() {
        let input = "reach me at alice@example.com or 555-123-4567; ssn 123-45-6789; card 4111 1111 1111 1111";
        let got = redact_text(input);
        assert!(!got.contains("alice@example.com"));
        assert!(!got.contains("123-45-6789"));
        assert!(!got.contains("555-123-4567"));
        assert!(!got.contains("4111"));
        assert!(got.contains(MASK));
    }

    #[test]
    fn skips_invalid_luhn_sequences() {
        let input = "not a card 4111 1111 1111 1112";
        let got = redact_text(input);
        assert!(got.contains("4111 1111 1111 1112"));
    }

    #[test]
    fn redacts_nested_json_strings() {
        let input = br#"{"content":{"text":"mail bob@test.org"}}"#;
        let got = redact_json_bytes(input).expect("json");
        let value: Value = serde_json::from_slice(&got).expect("parse");
        let text = value["content"]["text"].as_str().expect("text");
        assert!(!text.contains("bob@test.org"));
        assert!(text.contains(MASK));
    }
}
