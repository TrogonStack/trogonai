//! Re-signing of AWS SigV4 requests after sentinel substitution.
//!
//! Detection runs on the request as the client sent it, because it keys on the
//! sentinel access key id that substitution then replaces. A header-signed
//! request whose sentinel belongs to a linked pair that injects at the TLS
//! target gets a `ResignPlan`; once substitution has produced the request
//! that goes upstream, `ResignPlan::apply` signs that request with the real
//! secret, keeping the client's credential scope, signed header set and
//! `X-Amz-Date`. Request shapes that cannot be re-signed (streaming payloads,
//! presigned URLs, SigV4a) are refused unless a [`Sigv4Policies`] entry lets
//! them through unsigned for the real credential.
//!
//! Real values never reach a log, an error or a violation: refusals are plain
//! tags and the plan only ever hands them to the signer.

use std::sync::Arc;

use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::SecretName;
use trogon_sandbox_contract::{EgressViolationReason, UnsupportedSigv4Shape};
use trogon_sandbox_protocol::{RealValue, RequestSite, Sentinel, SentinelRegistry};

use crate::aws_sigv4::{
    PayloadHash, SignedHeaders, SigningInput, SigningScope, Sigv4Kind, detect, header_value, parse_authorization, sign,
};
use crate::header_syntax::{fold_header_name, is_header_value};
use crate::status_code::StatusCode;

mod constants;
use constants::{AMZ_CONTENT_SHA256, AMZ_DATE, AMZ_SECURITY_TOKEN, AUTHORIZATION, CONNECTION, HOP_BY_HOP};

/// Whether `name` is still on the request once the hop-by-hop headers have been
/// stripped, which is the set the SigV4 plan checks the signed headers against.
fn survives_hop_by_hop_strip(headers: &[(String, String)], name: &str) -> bool {
    let named_by_connection = headers
        .iter()
        .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(CONNECTION))
        .flat_map(|(_, value)| value.split(','))
        .any(|token| fold_header_name(token.trim()) == fold_header_name(name));
    !named_by_connection
        && !HOP_BY_HOP
            .iter()
            .any(|hop| fold_header_name(hop) == fold_header_name(name))
}

/// The largest body the proxy buffers to hash and re-sign a request whose
/// signature covers the body's SHA-256. A request signed with
/// `UNSIGNED-PAYLOAD` carries no body hash and is not held to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sigv4BodyLimit(usize);

impl Sigv4BodyLimit {
    pub const DEFAULT: Self = Self(64 * 1024 * 1024);

    pub fn new(bytes: usize) -> Self {
        Self(bytes)
    }

    pub fn bytes(self) -> usize {
        self.0
    }
}

impl Default for Sigv4BodyLimit {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What happens to a request of a shape that cannot be re-signed. `Deny`
/// refuses it before anything is forwarded. `Passthrough` forwards it with
/// ordinary substitution only, so it fails upstream on the sentinel signature.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UnsupportedSigv4Policy {
    #[default]
    Deny,
    Passthrough,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sigv4Policies {
    pub streaming: UnsupportedSigv4Policy,
    pub presigned: UnsupportedSigv4Policy,
    pub sigv4a: UnsupportedSigv4Policy,
}

impl Sigv4Policies {
    fn for_shape(&self, shape: UnsupportedSigv4Shape) -> UnsupportedSigv4Policy {
        match shape {
            UnsupportedSigv4Shape::Streaming => self.streaming,
            UnsupportedSigv4Shape::Presigned => self.presigned,
            UnsupportedSigv4Shape::Sigv4a => self.sigv4a,
        }
    }
}

/// Why a request signed with a masked AWS credential is answered instead of
/// forwarded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Sigv4RefusalError {
    #[error(
        "AWS SigV4 streaming request uses a masked credential but cannot be re-signed by the sandbox proxy; denied by policy. Set credentials.sigv4.streaming to \"passthrough\" to forward it unmodified (it will fail upstream: the signature covers the masked placeholder)."
    )]
    Streaming,
    #[error(
        "AWS SigV4 presigned request uses a masked credential but cannot be re-signed by the sandbox proxy; denied by policy. Set credentials.sigv4.presigned to \"passthrough\" to forward it unmodified (it will fail upstream: the signature covers the masked placeholder)."
    )]
    Presigned,
    #[error(
        "AWS SigV4 sigv4a request uses a masked credential but cannot be re-signed by the sandbox proxy; denied by policy. Set credentials.sigv4.sigv4a to \"passthrough\" to forward it unmodified (it will fail upstream: the signature covers the masked placeholder)."
    )]
    Sigv4a,
    #[error(
        "AWS SigV4 request uses a masked credential but its Authorization header is malformed; the proxy cannot re-sign it."
    )]
    MalformedAuthorization,
    #[error(
        "AWS SigV4 request uses a masked credential but carries no x-amz-date header; the proxy cannot re-sign it."
    )]
    MissingDate,
    #[error(
        "AWS SigV4 request uses a masked credential but signed header \"{0}\" is missing; the proxy cannot re-sign it."
    )]
    MissingSignedHeader(String),
    #[error("AWS SigV4 re-signing failed: {0}")]
    ResignFailed(String),
    #[error(
        "AWS SigV4 request uses a masked credential and signs a literal body hash, so the proxy must buffer the body to re-sign it, but the body exceeds the {}-byte buffering limit; denied. Sign the payload as UNSIGNED-PAYLOAD to stream it without buffering, or use an unmasked credential to have the request forwarded untouched.",
        .0.bytes()
    )]
    BodyTooLarge(Sigv4BodyLimit),
}

impl Sigv4RefusalError {
    pub fn status(&self) -> StatusCode {
        StatusCode::FORBIDDEN
    }

    pub fn violation_reason(&self) -> EgressViolationReason {
        match self {
            Self::Streaming => EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Streaming),
            Self::Presigned => EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Presigned),
            Self::Sigv4a => EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Sigv4a),
            Self::MalformedAuthorization
            | Self::MissingDate
            | Self::MissingSignedHeader(_)
            | Self::ResignFailed(_)
            | Self::BodyTooLarge(_) => EgressViolationReason::SecretSigv4Unsignable,
        }
    }

    fn unsupported(kind: Sigv4Kind) -> Option<(Self, UnsupportedSigv4Shape)> {
        match kind {
            Sigv4Kind::Header => None,
            Sigv4Kind::Streaming => Some((Self::Streaming, UnsupportedSigv4Shape::Streaming)),
            Sigv4Kind::Presigned => Some((Self::Presigned, UnsupportedSigv4Shape::Presigned)),
            Sigv4Kind::Sigv4a => Some((Self::Sigv4a, UnsupportedSigv4Shape::Sigv4a)),
        }
    }
}

/// A refused request together with the binding whose credential it used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PairRefusal {
    pub(crate) refusal: Sigv4RefusalError,
    pub(crate) binding: SecretName,
}

#[derive(Clone)]
struct PairEntry {
    binding: SecretName,
    access_key_id_sentinel: Sentinel,
    access_key_id: RealValue,
    secret_access_key: RealValue,
    session_token: Option<RealValue>,
}

/// The linked AWS pairs that inject at one TLS target.
#[derive(Clone, Default)]
pub(crate) struct AwsPairs {
    entries: Arc<[PairEntry]>,
    policies: Sigv4Policies,
}

impl AwsPairs {
    pub(crate) fn for_target(
        registry: &SentinelRegistry,
        host: &Host,
        port: u16,
        site: Option<RequestSite<'_>>,
        policies: Sigv4Policies,
    ) -> Self {
        let entries = registry
            .aws_pairs_for_request(host, port, site)
            .map(|pair| PairEntry {
                binding: pair.binding.clone(),
                access_key_id_sentinel: pair.access_key_id_sentinel.clone(),
                access_key_id: pair.access_key_id.clone(),
                secret_access_key: pair.secret_access_key.clone(),
                session_token: pair.session_token.cloned(),
            })
            .collect();
        Self { entries, policies }
    }

    /// Decides what to do with a request as the client sent it. `None` means it
    /// is not this module's to handle: no SigV4 signature, an access key id
    /// that is not a linked sentinel (a real, unmasked credential included), or
    /// a passthrough policy.
    pub(crate) fn plan(&self, headers: &[(String, String)], target: &str) -> Result<Option<ResignPlan>, PairRefusal> {
        if self.entries.is_empty() {
            return Ok(None);
        }
        let Some(detected) = detect(headers, target) else {
            return Ok(None);
        };
        let Some(pair) = self
            .entries
            .iter()
            .find(|pair| pair.access_key_id_sentinel.as_str() == detected.access_key_id)
        else {
            return Ok(None);
        };
        let refuse = |refusal| PairRefusal {
            refusal,
            binding: pair.binding.clone(),
        };

        if let Some((refusal, shape)) = Sigv4RefusalError::unsupported(detected.kind) {
            return match self.policies.for_shape(shape) {
                UnsupportedSigv4Policy::Passthrough => Ok(None),
                UnsupportedSigv4Policy::Deny => Err(refuse(refusal)),
            };
        }

        let authorizations = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(AUTHORIZATION))
            .count();
        let parsed = header_value(headers, AUTHORIZATION)
            .filter(|_| authorizations == 1)
            .and_then(parse_authorization)
            .ok_or_else(|| refuse(Sigv4RefusalError::MalformedAuthorization))?;
        let amz_date = header_value(headers, AMZ_DATE)
            .ok_or_else(|| refuse(Sigv4RefusalError::MissingDate))?
            .to_string();

        for name in parsed.signed_headers.iter() {
            let supplied = name == "host" || (name == AMZ_SECURITY_TOKEN && pair.session_token.is_some());
            let present = survives_hop_by_hop_strip(headers, name) && header_value(headers, name).is_some();
            if !supplied && !present {
                return Err(refuse(Sigv4RefusalError::MissingSignedHeader(name.to_string())));
            }
        }
        let signed_headers = if pair.session_token.is_some() {
            parsed.signed_headers.with(AMZ_SECURITY_TOKEN)
        } else {
            parsed.signed_headers
        };

        let payload = match header_value(headers, AMZ_CONTENT_SHA256) {
            Some(hash) if hash == crate::aws_sigv4::UNSIGNED_PAYLOAD => PayloadSource::Unsigned,
            _ => PayloadSource::Body,
        };
        Ok(Some(ResignPlan {
            pair: pair.clone(),
            scope: parsed.scope,
            signed_headers,
            amz_date,
            payload,
        }))
    }
}

#[derive(Clone, Copy)]
enum PayloadSource {
    Unsigned,
    Body,
}

/// A re-sign decided on the client's request, applied to the substituted one.
pub(crate) struct ResignPlan {
    pair: PairEntry,
    scope: SigningScope,
    signed_headers: SignedHeaders,
    amz_date: String,
    payload: PayloadSource,
}

impl ResignPlan {
    pub(crate) fn binding(&self) -> &SecretName {
        &self.pair.binding
    }

    /// Whether the signature covers a hash of the body, so the body has to be
    /// available (decoded from its transfer framing) to apply the plan.
    pub(crate) fn hashes_body(&self) -> bool {
        matches!(self.payload, PayloadSource::Body)
    }

    /// Signs the request that goes upstream. `headers` are the post
    /// substitution headers, `body` the payload bytes as the upstream will
    /// decode them. The session token joins the request and the signed set, a
    /// literal `x-amz-content-sha256` is pinned to the recomputed hash, and the
    /// Authorization header is replaced.
    pub(crate) fn apply(
        &self,
        method: &str,
        target: &str,
        headers: &mut Vec<(String, String)>,
        body: &[u8],
    ) -> Result<(), Sigv4RefusalError> {
        if let Some(token) = &self.pair.session_token {
            if !is_header_value(token.expose()) {
                return Err(Sigv4RefusalError::ResignFailed(
                    "the session token holds a character a header cannot carry".to_string(),
                ));
            }
            set_header(headers, AMZ_SECURITY_TOKEN, token.expose());
        }
        let payload_hash = match self.payload {
            PayloadSource::Unsigned => PayloadHash::Unsigned,
            PayloadSource::Body => PayloadHash::of(body),
        };
        if matches!(self.payload, PayloadSource::Body) && header_value(headers, AMZ_CONTENT_SHA256).is_some() {
            set_header(headers, AMZ_CONTENT_SHA256, payload_hash.as_str());
        }
        let authorization = sign(
            &SigningInput {
                method,
                target,
                headers,
                signed_headers: &self.signed_headers,
                payload_hash: &payload_hash,
                amz_date: &self.amz_date,
                scope: &self.scope,
            },
            &self.pair.access_key_id,
            &self.pair.secret_access_key,
        )
        .map_err(|error| Sigv4RefusalError::ResignFailed(error.to_string()))?;
        set_header(headers, AUTHORIZATION, &authorization);
        Ok(())
    }
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    let mut seen = false;
    headers.retain_mut(|(candidate, current)| {
        if !candidate.eq_ignore_ascii_case(name) {
            return true;
        }
        if seen {
            return false;
        }
        seen = true;
        *current = value.to_string();
        true
    });
    if !seen {
        headers.push((name.to_string(), value.to_string()));
    }
}
