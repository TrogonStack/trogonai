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
mod tests;
