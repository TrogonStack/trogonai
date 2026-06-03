//! Request signers. Two transports:
//!
//! * [`HttpRequestSigner`] — produces RFC 9421 `Signature-Input` / `Signature`
//!   plus `Content-Digest` and `AAuth-Token` headers.
//! * [`NatsRequestSigner`] — produces the Trogon NATS envelope
//!   (`AAuth-Token`, `AAuth-Sig-Input`, `AAuth-Sig`, `AAuth-Sig-Created`,
//!   `AAuth-Sig-Nonce`).
//!
//! Both signers consume an [`AgentKeypair`] and the agent's bootstrap JWT.
//! The signers do not own the agent JWT lifecycle — callers (or the
//! [`crate::retry`] helpers) refresh that token separately.

use std::collections::BTreeMap;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use trogon_identity_types::aauth::{NatsSignatureEnvelope, headers};

use crate::keypair::AgentKeypair;

/// Compute the canonical Content-Digest value (sha-256 in structured field form).
#[must_use]
pub fn content_digest_sha256(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    format!("sha-256=:{}:", URL_SAFE_NO_PAD.encode(digest))
}

/// Result of [`NatsRequestSigner::sign`]: a set of headers ready to attach to
/// an outbound NATS message.
#[derive(Debug, Clone)]
pub struct SignedNatsRequest {
    pub headers: BTreeMap<String, String>,
}

impl SignedNatsRequest {
    /// Convenience: produce an iterable list of (name, value) pairs.
    pub fn into_pairs(self) -> Vec<(String, String)> {
        self.headers.into_iter().collect()
    }
}

/// Signs outbound NATS messages with the AAuth envelope.
pub struct NatsRequestSigner<'a> {
    pub keypair: &'a AgentKeypair,
    pub agent_jwt: &'a str,
}

impl<'a> NatsRequestSigner<'a> {
    #[must_use]
    pub fn new(keypair: &'a AgentKeypair, agent_jwt: &'a str) -> Self {
        Self { keypair, agent_jwt }
    }

    /// Build the AAuth headers for a single request.
    ///
    /// * `created` should be epoch seconds (callers pass the agent's clock).
    /// * `nonce` must be unique per request within the verifier's replay window
    ///   (use [`uuid::Uuid::now_v7`] or similar).
    pub fn sign(
        &self,
        subject: &str,
        reply: Option<&str>,
        payload: &[u8],
        created: i64,
        nonce: &str,
    ) -> SignedNatsRequest {
        let content_digest = content_digest_sha256(payload);
        let sig_input = "(\"@subject\" \"@reply\" \"content-digest\" \"aauth-token\" \"aauth-sig-created\" \"aauth-sig-nonce\")".to_string();

        let envelope = NatsSignatureEnvelope {
            token: self.agent_jwt.to_string(),
            sig_input: sig_input.clone(),
            sig: String::new(),
            created,
            nonce: nonce.to_string(),
            content_digest: content_digest.clone(),
        };
        let canonical = envelope.canonical_base(subject, reply, self.keypair.jkt());
        let sig = self.keypair.sign_raw(canonical.as_bytes());

        let mut headers_map = BTreeMap::new();
        headers_map.insert(headers::NATS_TOKEN.into(), self.agent_jwt.to_string());
        headers_map.insert(headers::NATS_SIG_INPUT.into(), sig_input);
        headers_map.insert(headers::NATS_SIG.into(), sig);
        headers_map.insert(headers::NATS_SIG_CREATED.into(), created.to_string());
        headers_map.insert(headers::NATS_SIG_NONCE.into(), nonce.to_string());
        headers_map.insert(headers::CONTENT_DIGEST.into(), content_digest);

        SignedNatsRequest { headers: headers_map }
    }
}

/// Signs outbound HTTP requests with RFC 9421 headers.
pub struct HttpRequestSigner<'a> {
    pub keypair: &'a AgentKeypair,
    pub agent_jwt: &'a str,
}

#[derive(Debug, Clone)]
pub struct SignedHttpHeaders {
    pub signature_input: String,
    pub signature: String,
    pub content_digest: String,
    pub aauth_token: String,
}

impl<'a> HttpRequestSigner<'a> {
    #[must_use]
    pub fn new(keypair: &'a AgentKeypair, agent_jwt: &'a str) -> Self {
        Self { keypair, agent_jwt }
    }

    /// Sign an HTTP request. Only covers a minimal field set sufficient for the
    /// MCP gateway path: `@method`, `@path`, `@authority`, `content-digest`,
    /// `aauth-token`.
    ///
    /// * `method` — request method (uppercased)
    /// * `path` — request path (no query for now; query support to follow)
    /// * `authority` — request `Host` header value
    /// * `body` — request body bytes (may be empty)
    /// * `created` — epoch seconds
    /// * `nonce` — random per-request nonce
    pub fn sign(
        &self,
        method: &str,
        path: &str,
        authority: &str,
        body: &[u8],
        created: i64,
        nonce: &str,
    ) -> SignedHttpHeaders {
        let content_digest = content_digest_sha256(body);
        let covered = "(\"@method\" \"@path\" \"@authority\" \"content-digest\" \"aauth-token\")";
        let sig_params = format!(
            "{covered};created={created};keyid=\"{kid}\";nonce=\"{nonce}\"",
            kid = self.keypair.jkt()
        );
        let base = format!(
            concat!(
                "\"@method\": {method}\n",
                "\"@path\": {path}\n",
                "\"@authority\": {authority}\n",
                "\"content-digest\": {digest}\n",
                "\"aauth-token\": {token}\n",
                "\"@signature-params\": {params}",
            ),
            method = method,
            path = path,
            authority = authority,
            digest = content_digest,
            token = self.agent_jwt,
            params = sig_params,
        );
        let sig = self.keypair.sign_raw(base.as_bytes());
        SignedHttpHeaders {
            signature_input: format!("sig1={sig_params}"),
            signature: format!("sig1=:{sig}:"),
            content_digest,
            aauth_token: self.agent_jwt.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nats_signer_emits_required_headers() {
        let kp = AgentKeypair::generate().unwrap();
        let signer = NatsRequestSigner::new(&kp, "tok");
        let signed = signer.sign("foo.bar", Some("_INBOX.1"), b"payload", 1_700_000_000, "n-1");
        for required in [
            headers::NATS_TOKEN,
            headers::NATS_SIG_INPUT,
            headers::NATS_SIG,
            headers::NATS_SIG_CREATED,
            headers::NATS_SIG_NONCE,
            headers::CONTENT_DIGEST,
        ] {
            assert!(signed.headers.contains_key(required), "missing {required}");
        }
        // Same inputs produce same digest deterministically.
        let again = signer.sign("foo.bar", Some("_INBOX.1"), b"payload", 1_700_000_000, "n-1");
        assert_eq!(
            signed.headers[headers::CONTENT_DIGEST],
            again.headers[headers::CONTENT_DIGEST]
        );
    }

    #[test]
    fn http_signer_emits_rfc9421_fields() {
        let kp = AgentKeypair::generate().unwrap();
        let signer = HttpRequestSigner::new(&kp, "tok");
        let s = signer.sign("POST", "/mcp", "gw.example", b"{}", 1_700_000_000, "n-1");
        assert!(s.signature_input.starts_with("sig1="));
        assert!(s.signature.starts_with("sig1=:"));
        assert!(s.content_digest.starts_with("sha-256=:"));
    }
}
