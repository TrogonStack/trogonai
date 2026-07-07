//! Third-party login wire types per draft section "Third-Party Login": the
//! `login_endpoint` query parameters and login flow. The login endpoint is a
//! GET with query parameters (not a JSON body), so this models the parameter
//! set plus a minimal query-string renderer/parser rather than a serde JSON body.

use serde::{Deserialize, Serialize};

/// Query parameters accepted by an agent's or resource's `login_endpoint`, per
/// "Login Endpoint".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub ps: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_path: Option<String>,
}

impl LoginRequest {
    #[must_use]
    pub fn new(ps: impl Into<String>) -> Self {
        Self {
            ps: ps.into(),
            login_hint: None,
            domain_hint: None,
            tenant: None,
            start_path: None,
        }
    }

    /// Render as a `login_endpoint` query string (without the leading `?`), per
    /// the draft's example login URL.
    #[must_use]
    pub fn to_query_string(&self) -> String {
        let mut parts = vec![format!("ps={}", urlencode(&self.ps))];
        if let Some(v) = &self.tenant {
            parts.push(format!("tenant={}", urlencode(v)));
        }
        if let Some(v) = &self.login_hint {
            parts.push(format!("login_hint={}", urlencode(v)));
        }
        if let Some(v) = &self.domain_hint {
            parts.push(format!("domain_hint={}", urlencode(v)));
        }
        if let Some(v) = &self.start_path {
            parts.push(format!("start_path={}", urlencode(v)));
        }
        parts.join("&")
    }

    /// Parse a `login_endpoint` query string (without the leading `?`).
    #[must_use]
    pub fn parse_query_string(raw: &str) -> Option<Self> {
        let mut ps: Option<String> = None;
        let mut login_hint: Option<String> = None;
        let mut domain_hint: Option<String> = None;
        let mut tenant: Option<String> = None;
        let mut start_path: Option<String> = None;
        for pair in raw.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=')?;
            let value = urldecode(v);
            match k {
                "ps" => ps = Some(value),
                "login_hint" => login_hint = Some(value),
                "domain_hint" => domain_hint = Some(value),
                "tenant" => tenant = Some(value),
                "start_path" => start_path = Some(value),
                _ => {}
            }
        }
        Some(LoginRequest {
            ps: ps?,
            login_hint,
            domain_hint,
            tenant,
            start_path,
        })
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Decode the two hex digits from raw bytes, never by slicing the
        // &str: a percent sign followed by a multi-byte code point would
        // put the slice boundary inside a character and panic.
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_request_query_string_round_trip() {
        let req = LoginRequest {
            ps: "https://ps.example".into(),
            tenant: Some("corp".into()),
            login_hint: Some("user@corp.example".into()),
            domain_hint: None,
            start_path: Some("/projects/tokyo-trip".into()),
        };
        let query = req.to_query_string();
        let parsed = LoginRequest::parse_query_string(&query).unwrap();
        assert_eq!(parsed, req);
    }

    #[test]
    fn login_request_matches_draft_example_url() {
        let query = "ps=https%3A%2F%2Fps.example&tenant=corp&login_hint=user%40corp.example&start_path=%2Fprojects%2Ftokyo-trip";
        let parsed = LoginRequest::parse_query_string(query).unwrap();
        assert_eq!(parsed.ps, "https://ps.example");
        assert_eq!(parsed.tenant.as_deref(), Some("corp"));
        assert_eq!(parsed.login_hint.as_deref(), Some("user@corp.example"));
        assert_eq!(parsed.start_path.as_deref(), Some("/projects/tokyo-trip"));
    }

    #[test]
    fn login_request_serde_round_trip() {
        let req = LoginRequest::new("https://ps.example");
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json, serde_json::json!({"ps": "https://ps.example"}));
        let back: LoginRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn login_request_minimal_query_string_only_ps() {
        let req = LoginRequest::new("https://ps.example");
        assert_eq!(req.to_query_string(), "ps=https%3A%2F%2Fps.example");
    }

    #[test]
    fn parse_query_string_survives_percent_before_multibyte_character() {
        // A percent sign directly followed by a multi-byte code point used
        // to panic via a mid-character &str slice in urldecode.
        let parsed = LoginRequest::parse_query_string("ps=%\u{20ac}&login_hint=a%zzb");
        let parsed = parsed.expect("malformed escapes fall through as literals");
        assert_eq!(parsed.ps, "%\u{20ac}");
        assert_eq!(parsed.login_hint.as_deref(), Some("a%zzb"));
    }

    #[test]
    fn to_query_string_includes_domain_hint_when_present() {
        let req = LoginRequest {
            ps: "https://ps.example".into(),
            login_hint: None,
            domain_hint: Some("corp.example".into()),
            tenant: None,
            start_path: None,
        };
        let query = req.to_query_string();
        assert_eq!(query, "ps=https%3A%2F%2Fps.example&domain_hint=corp.example");
        let parsed = LoginRequest::parse_query_string(&query).unwrap();
        assert_eq!(parsed, req);
    }

    #[test]
    fn parse_query_string_skips_empty_pairs_from_stray_ampersands() {
        let parsed = LoginRequest::parse_query_string("ps=https%3A%2F%2Fps.example&&").unwrap();
        assert_eq!(parsed.ps, "https://ps.example");
    }

    #[test]
    fn parse_query_string_ignores_unknown_parameters() {
        let parsed = LoginRequest::parse_query_string("ps=https%3A%2F%2Fps.example&unknown=1").unwrap();
        assert_eq!(parsed.ps, "https://ps.example");
    }
}
