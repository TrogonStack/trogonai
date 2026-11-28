//! AWS Signature Version 4 protocol code for the TLS-terminating proxy.
//!
//! A masked AWS credential hands the sandbox a sentinel access key id and
//! secret, so any SigV4 signature the client computes is an HMAC over the
//! sentinel secret: substituting the access key id in the credential scope
//! cannot fix the signature. The proxy therefore re-signs, keeping the
//! client's credential scope, signed header set and `X-Amz-Date` and
//! recomputing the signature with the real secret over the request as it goes
//! upstream.
//!
//! This module is pure protocol code: request classification, Authorization
//! parsing, canonical request construction and signing, checked against
//! known-answer tests. The pair lookup and request
//! rewriting live in [`crate::sigv4_resign`].

use sha2::{Digest, Sha256};
use trogon_sandbox_protocol::RealValue;

mod constants;
use constants::{HMAC_BLOCK, SCOPE_TERMINATOR, STREAMING_PREFIX};
pub(crate) use constants::{SIGV4_ALGORITHM, SIGV4A_ALGORITHM, UNSIGNED_PAYLOAD};

/// How a request references SigV4 authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sigv4Kind {
    /// `Authorization: AWS4-HMAC-SHA256 ...`, the shape that can be re-signed.
    Header,
    /// `x-amz-content-sha256: STREAMING-*`, an aws-chunked body whose chunk
    /// signatures chain off the seed signature.
    Streaming,
    /// The signature lives in the query (`X-Amz-Signature`). Classified
    /// whenever the Authorization header does not classify itself, so a junk
    /// Authorization value cannot hide a presigned query.
    Presigned,
    /// `Authorization: AWS4-ECDSA-P256-SHA256 ...`, an asymmetric signature
    /// with no shared-key HMAC to recompute.
    Sigv4a,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sigv4Detection {
    pub(crate) kind: Sigv4Kind,
    pub(crate) access_key_id: String,
}

pub(crate) fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Classifies a request's SigV4 shape from its headers and request-target,
/// without looking at the body. `None` when it carries no recognisable
/// signature.
pub(crate) fn detect(headers: &[(String, String)], target: &str) -> Option<Sigv4Detection> {
    if let Some(auth) = header_value(headers, "authorization") {
        let algorithm = auth.split_whitespace().next();
        if algorithm == Some(SIGV4A_ALGORITHM)
            && let Some(access_key_id) = credential_access_key_id(auth)
        {
            return Some(Sigv4Detection {
                kind: Sigv4Kind::Sigv4a,
                access_key_id,
            });
        }
        if algorithm == Some(SIGV4_ALGORITHM)
            && let Some(access_key_id) = credential_access_key_id(auth)
        {
            let streaming =
                header_value(headers, "x-amz-content-sha256").is_some_and(|hash| hash.starts_with(STREAMING_PREFIX));
            return Some(Sigv4Detection {
                kind: if streaming {
                    Sigv4Kind::Streaming
                } else {
                    Sigv4Kind::Header
                },
                access_key_id,
            });
        }
    }

    let (_, query) = target.split_once('?')?;
    let params = form_pairs(query);
    let param = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    if param("X-Amz-Algorithm") != Some(SIGV4_ALGORITHM) || param("X-Amz-Signature").is_none() {
        return None;
    }
    let access_key_id = param("X-Amz-Credential")?.split('/').next()?;
    (!access_key_id.is_empty()).then(|| Sigv4Detection {
        kind: Sigv4Kind::Presigned,
        access_key_id: access_key_id.to_string(),
    })
}

fn auth_field<'a>(auth: &'a str, name: &str) -> Option<&'a str> {
    auth.split(|c: char| c == ',' || c.is_whitespace())
        .find_map(|token| token.strip_prefix(name))
        .filter(|value| !value.is_empty())
}

fn credential_access_key_id(auth: &str) -> Option<String> {
    let (access_key_id, _) = auth_field(auth, "Credential=")?.split_once('/')?;
    (!access_key_id.is_empty()).then(|| access_key_id.to_string())
}

/// The credential scope a signature is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SigningScope {
    pub(crate) date: String,
    pub(crate) region: String,
    pub(crate) service: String,
}

impl SigningScope {
    fn render(&self) -> String {
        format!("{}/{}/{}/{SCOPE_TERMINATOR}", self.date, self.region, self.service)
    }
}

/// Lowercase signed header names, sorted, without duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignedHeaders(Vec<String>);

impl SignedHeaders {
    pub(crate) fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut names: Vec<String> = names
            .into_iter()
            .map(|name| name.as_ref().to_ascii_lowercase())
            .collect();
        names.sort();
        names.dedup();
        Self(names)
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|candidate| candidate == name)
    }

    pub(crate) fn with(mut self, name: &str) -> Self {
        if !self.contains(name) {
            self.0.push(name.to_string());
            self.0.sort();
        }
        self
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    fn joined(&self) -> String {
        self.0.join(";")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedAuthorization {
    pub(crate) access_key_id: String,
    pub(crate) scope: SigningScope,
    pub(crate) signed_headers: SignedHeaders,
}

/// Parses `AWS4-HMAC-SHA256 Credential=..., SignedHeaders=..., Signature=...`.
/// `None` when a component is missing or the credential scope is not the
/// five-part `<akid>/<date>/<region>/<service>/aws4_request` form.
pub(crate) fn parse_authorization(auth: &str) -> Option<ParsedAuthorization> {
    if auth.split_whitespace().next() != Some(SIGV4_ALGORITHM) {
        return None;
    }
    let credential = auth_field(auth, "Credential=")?;
    let signed_headers = auth_field(auth, "SignedHeaders=")?;
    let signature = auth_field(auth, "Signature=")?;
    if !signature.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let parts: Vec<&str> = credential.split('/').collect();
    let [access_key_id, date, region, service, terminator] = parts[..] else {
        return None;
    };
    if terminator != SCOPE_TERMINATOR
        || [access_key_id, date, region, service]
            .iter()
            .any(|part| part.is_empty())
    {
        return None;
    }
    Some(ParsedAuthorization {
        access_key_id: access_key_id.to_string(),
        scope: SigningScope {
            date: date.to_string(),
            region: region.to_string(),
            service: service.to_string(),
        },
        signed_headers: SignedHeaders::new(signed_headers.split(';')),
    })
}

/// The payload hash a signature covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PayloadHash {
    Unsigned,
    Sha256(String),
}

impl PayloadHash {
    pub(crate) fn of(body: &[u8]) -> Self {
        Self::Sha256(hex::encode(Sha256::digest(body)))
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Unsigned => UNSIGNED_PAYLOAD,
            Self::Sha256(hex) => hex,
        }
    }
}

/// RFC 3986 percent-encoding: everything except `A-Za-z0-9-_.~` is encoded.
pub(crate) fn uri_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn form_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            let decode = |text: &str| {
                let spaced = text.replace('+', " ");
                percent_decode(&spaced).unwrap_or(spaced)
            };
            (decode(key), decode(value))
        })
        .collect()
}

/// Canonical URI: S3 signs the raw path verbatim, every other service signs
/// the dot-segment-resolved path percent-encoded once more on top of its wire
/// encoding, with `/` preserved.
pub(crate) fn canonical_uri(raw_path: &str, service: &str) -> String {
    if raw_path.is_empty() {
        return "/".to_string();
    }
    if service == "s3" {
        return raw_path.to_string();
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in raw_path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let trailing = if !segments.is_empty() && raw_path.ends_with('/') {
        "/"
    } else {
        ""
    };
    let encoded: Vec<String> = segments.into_iter().map(uri_encode).collect();
    format!("/{}{trailing}", encoded.join("/"))
}

/// Canonical query string: keys and values are decoded then strictly
/// re-encoded, pairs are sorted by encoded key then value. A literal `+` is a
/// plus, not a space.
pub(crate) fn canonical_query_string(raw_query: &str) -> String {
    let mut pairs: Vec<(String, String)> = raw_query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            let encode = |text: &str| uri_encode(&percent_decode(text).unwrap_or(text.to_string()));
            (encode(key), encode(value))
        })
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn trimall(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_run = false;
    for c in value.trim().chars() {
        if c == ' ' || c == '\t' {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("signed header '{0}' is missing from the request")]
pub(crate) struct MissingSignedHeaderError(pub(crate) String);

pub(crate) struct SigningInput<'a> {
    pub(crate) method: &'a str,
    pub(crate) target: &'a str,
    pub(crate) headers: &'a [(String, String)],
    pub(crate) signed_headers: &'a SignedHeaders,
    pub(crate) payload_hash: &'a PayloadHash,
    pub(crate) amz_date: &'a str,
    pub(crate) scope: &'a SigningScope,
}

/// Builds the SigV4 canonical request. A signed header other than `host` that
/// is absent from the request is an error: signing an empty value the
/// upstream never sees would produce a signature it rejects confusingly.
pub(crate) fn canonical_request(input: &SigningInput<'_>) -> Result<String, MissingSignedHeaderError> {
    let (path, query) = input.target.split_once('?').unwrap_or((input.target, ""));
    let mut header_lines = String::new();
    for name in input.signed_headers.iter() {
        let values: Vec<String> = input
            .headers
            .iter()
            .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| trimall(value))
            .collect();
        if values.is_empty() {
            return Err(MissingSignedHeaderError(name.to_string()));
        }
        header_lines.push_str(&format!("{name}:{}\n", values.join(",")));
    }
    Ok([
        input.method.to_ascii_uppercase(),
        canonical_uri(path, &input.scope.service),
        canonical_query_string(query),
        header_lines,
        input.signed_headers.joined(),
        input.payload_hash.as_str().to_string(),
    ]
    .join("\n"))
}

pub(crate) fn string_to_sign(canonical_request: &str, amz_date: &str, scope: &SigningScope) -> String {
    format!(
        "{SIGV4_ALGORITHM}\n{amz_date}\n{}\n{}",
        scope.render(),
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    )
}

/// HMAC-SHA256 (RFC 2104); a key longer than the block is hashed first.
fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut block = [0u8; HMAC_BLOCK];
    if key.len() > HMAC_BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner_pad = block.map(|byte| byte ^ 0x36);
    let outer_pad = block.map(|byte| byte ^ 0x5c);
    let inner = Sha256::new().chain_update(inner_pad).chain_update(data).finalize();
    Sha256::new()
        .chain_update(outer_pad)
        .chain_update(inner)
        .finalize()
        .to_vec()
}

fn signing_key(secret_access_key: &RealValue, scope: &SigningScope) -> Vec<u8> {
    let date = hmac(
        format!("AWS4{}", secret_access_key.expose()).as_bytes(),
        scope.date.as_bytes(),
    );
    let region = hmac(&date, scope.region.as_bytes());
    let service = hmac(&region, scope.service.as_bytes());
    hmac(&service, SCOPE_TERMINATOR.as_bytes())
}

/// Signs the request and returns the Authorization header value, which carries
/// the access key id and the signature.
pub(crate) fn sign(
    input: &SigningInput<'_>,
    access_key_id: &RealValue,
    secret_access_key: &RealValue,
) -> Result<String, MissingSignedHeaderError> {
    let canonical_request = canonical_request(input)?;
    let string_to_sign = string_to_sign(&canonical_request, input.amz_date, input.scope);
    let signature = hex::encode(hmac(
        &signing_key(secret_access_key, input.scope),
        string_to_sign.as_bytes(),
    ));
    Ok(format!(
        "{SIGV4_ALGORITHM} Credential={}/{}, SignedHeaders={}, Signature={signature}",
        access_key_id.expose(),
        input.scope.render(),
        input.signed_headers.joined(),
    ))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod suite_tests;
