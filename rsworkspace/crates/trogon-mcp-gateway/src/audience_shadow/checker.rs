//! Compare inbound JWT `aud` against the expected mesh audience for this hop.

use std::time::{SystemTime, UNIX_EPOCH};

use super::aud_format::compute_expected_aud;
use super::audit_sink::{AudMismatchEnvelope, AudienceAuditSink};
use super::errors::AudienceShadowError;
use super::mode::AudienceShadowMode;
use super::AudienceShadowOutcome;

/// Correlation fields copied into [`AudMismatchEnvelope`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudienceCheckContext {
    pub tenant_id: String,
    pub caller_sub: String,
    pub request_id: String,
}

/// Inbound JWT `aud` claim shape (string or string array per RFC 7519).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimAud<'a> {
    Missing,
    Single(&'a str),
    List(&'a [&'a str]),
}

impl<'a> ClaimAud<'a> {
    fn observed_values(&self) -> Vec<String> {
        match self {
            Self::Missing => Vec::new(),
            Self::Single(aud) if aud.is_empty() => Vec::new(),
            Self::Single(aud) => vec![(*aud).to_string()],
            Self::List(values) => values
                .iter()
                .filter(|aud| !aud.is_empty())
                .map(|aud| (*aud).to_string())
                .collect(),
        }
    }

    fn matches_expected(&self, expected_aud: &str) -> bool {
        match self {
            Self::Missing => false,
            Self::Single(aud) => aud == &expected_aud,
            Self::List(values) => values.iter().any(|aud| *aud == expected_aud),
        }
    }

    fn is_missing(&self) -> bool {
        matches!(self, Self::Missing) || self.observed_values().is_empty()
    }
}

/// Audience validator for shadow and enforce modes.
pub struct AudienceShadowChecker;

impl AudienceShadowChecker {
    /// Compare `claim_aud` to `compute_expected_aud(tenant, server_id)` under `mode`.
    ///
    /// # Errors
    ///
    /// Returns [`AudienceShadowError`] when segments are malformed or `aud` is absent in
    /// shadow/enforce modes.
    pub fn check<S: AudienceAuditSink>(
        sink: Option<&S>,
        ctx: &AudienceCheckContext,
        claim_aud: ClaimAud<'_>,
        tenant: &str,
        server_id: &str,
        mode: AudienceShadowMode,
    ) -> Result<AudienceShadowOutcome, AudienceShadowError> {
        if mode == AudienceShadowMode::Off {
            return Ok(AudienceShadowOutcome::NoOp);
        }

        let expected_aud = compute_expected_aud(tenant, server_id)?;

        if claim_aud.is_missing() {
            return Err(AudienceShadowError::MissingAud);
        }

        if claim_aud.matches_expected(&expected_aud) {
            return Ok(AudienceShadowOutcome::Match);
        }

        let observed_aud = claim_aud.observed_values();
        if let Some(sink) = sink {
            sink.emit_mismatch(AudMismatchEnvelope {
                tenant_id: ctx.tenant_id.clone(),
                caller_sub: ctx.caller_sub.clone(),
                request_id: ctx.request_id.clone(),
                expected_aud: expected_aud.clone(),
                observed_aud: observed_aud.clone(),
                ts_unix_ms: unix_ms_now(),
            });
        }

        Ok(match mode {
            AudienceShadowMode::Shadow => AudienceShadowOutcome::ShadowMismatch,
            AudienceShadowMode::Enforce => AudienceShadowOutcome::EnforceMismatch,
            AudienceShadowMode::Off => AudienceShadowOutcome::NoOp,
        })
    }
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audience_shadow::audit_sink::RecordingAuditSink;

    fn ctx() -> AudienceCheckContext {
        AudienceCheckContext {
            tenant_id: "acme".into(),
            caller_sub: "agent:acme/oncall".into(),
            request_id: "req-1".into(),
        }
    }

    #[test]
    fn off_mode_short_circuits_on_mismatch() {
        let sink = RecordingAuditSink::new();
        let outcome = AudienceShadowChecker::check(
            Some(&sink),
            &ctx(),
            ClaimAud::Single("urn:trogon:mcp:backend:acme:other"),
            "acme",
            "github",
            AudienceShadowMode::Off,
        )
        .expect("off never errors");
        assert_eq!(outcome, AudienceShadowOutcome::NoOp);
        assert!(sink.drain().is_empty());
    }

    #[test]
    fn shadow_match_emits_nothing() {
        let sink = RecordingAuditSink::new();
        let expected = compute_expected_aud("acme", "github").expect("valid");
        let outcome = AudienceShadowChecker::check(
            Some(&sink),
            &ctx(),
            ClaimAud::Single(expected.as_str()),
            "acme",
            "github",
            AudienceShadowMode::Shadow,
        )
        .expect("match");
        assert_eq!(outcome, AudienceShadowOutcome::Match);
        assert!(sink.drain().is_empty());
    }

    #[test]
    fn shadow_mismatch_audits() {
        let sink = RecordingAuditSink::new();
        let expected = compute_expected_aud("acme", "github").expect("valid");
        let outcome = AudienceShadowChecker::check(
            Some(&sink),
            &ctx(),
            ClaimAud::Single("urn:trogon:mcp:backend:acme:other"),
            "acme",
            "github",
            AudienceShadowMode::Shadow,
        )
        .expect("mismatch");
        assert_eq!(outcome, AudienceShadowOutcome::ShadowMismatch);
        let records = sink.drain();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].expected_aud, expected);
        assert_eq!(records[0].observed_aud, vec!["urn:trogon:mcp:backend:acme:other"]);
        assert_eq!(records[0].tenant_id, "acme");
        assert_eq!(records[0].caller_sub, "agent:acme/oncall");
        assert_eq!(records[0].request_id, "req-1");
    }

    #[test]
    fn enforce_mismatch_audits() {
        let sink = RecordingAuditSink::new();
        let outcome = AudienceShadowChecker::check(
            Some(&sink),
            &ctx(),
            ClaimAud::Single("wrong"),
            "acme",
            "github",
            AudienceShadowMode::Enforce,
        )
        .expect("mismatch");
        assert_eq!(outcome, AudienceShadowOutcome::EnforceMismatch);
        assert_eq!(sink.drain().len(), 1);
    }

    #[test]
    fn array_aud_match_when_one_element_matches() {
        let sink = RecordingAuditSink::new();
        let expected = compute_expected_aud("acme", "github").expect("valid");
        let wrong = "urn:trogon:mcp:backend:acme:other";
        let values = [wrong, expected.as_str()];
        let outcome = AudienceShadowChecker::check(
            Some(&sink),
            &ctx(),
            ClaimAud::List(&values),
            "acme",
            "github",
            AudienceShadowMode::Shadow,
        )
        .expect("match in array");
        assert_eq!(outcome, AudienceShadowOutcome::Match);
        assert!(sink.drain().is_empty());
    }

    #[test]
    fn missing_aud_is_error_in_shadow() {
        let sink = RecordingAuditSink::new();
        let err = AudienceShadowChecker::check(
            Some(&sink),
            &ctx(),
            ClaimAud::Missing,
            "acme",
            "github",
            AudienceShadowMode::Shadow,
        )
        .expect_err("missing aud");
        assert_eq!(err, AudienceShadowError::MissingAud);
        assert!(sink.drain().is_empty());
    }
}
