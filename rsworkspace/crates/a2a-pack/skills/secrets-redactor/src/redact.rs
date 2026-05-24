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
    out = mask_aws_keys(&out);
    out = mask_github_pats(&out);
    out = mask_stripe_keys(&out);
    mask_bearer_jwts(&out)
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

fn mask_aws_keys(text: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\bAKIA[0-9A-Z]{16}\b").expect("aws key regex"));
    re.replace_all(text, MASK).into_owned()
}

fn mask_github_pats(text: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\bghp_[A-Za-z0-9_]{20,}\b").expect("github pat regex"));
    re.replace_all(text, MASK).into_owned()
}

fn mask_stripe_keys(text: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\bsk_live_[A-Za-z0-9]{16,}\b").expect("stripe live key regex")
    });
    re.replace_all(text, MASK).into_owned()
}

fn mask_bearer_jwts(text: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)\bBearer\s+eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b")
            .expect("bearer jwt regex")
    });
    re.replace_all(text, MASK).into_owned()
}

#[derive(Debug)]
pub enum RedactError {
    Json(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_common_secret_formats() {
        let stripe_key = format!("sk_{}_{}", "live", "000000000000000000000000");
        let input = format!(
            "aws AKIAIOSFODNN7EXAMPLE github ghp_1234567890abcdefghijklmnopqrstuvwxyz stripe {stripe_key} auth Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.signature"
        );
        let got = redact_text(&input);
        assert!(!got.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!got.contains("ghp_1234567890abcdefghijklmnopqrstuvwxyz"));
        assert!(!got.contains(&stripe_key));
        assert!(!got.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
        assert!(got.contains(MASK));
    }

    #[test]
    fn redacts_nested_json_strings() {
        let input = br#"{"text":"token ghp_abcdefghijklmnopqrstuvwxyz1234567890AB"}"#;
        let got = redact_json_bytes(input).expect("json");
        let value: Value = serde_json::from_slice(&got).expect("parse");
        let text = value["text"].as_str().expect("text");
        assert!(!text.contains("ghp_"));
        assert!(text.contains(MASK));
    }
}
