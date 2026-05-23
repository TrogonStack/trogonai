//! Phase 3 — Tier 3 WASM redaction over **`Message.parts`** / **`Artifact.parts`** (skill-keyed).
//!
//! **Status today:** outbound `parts[*]` pass through unchanged at gateway egress and push
//! materialization. This module is compile-only scaffolding for skill-id-keyed redaction programs
//! shipped in `a2a-pack` and hosted on the shared Wasmtime substrate.
//!
//! **Target:** when materializing outbound [`Message.parts`](https://github.com/google/A2A/blob/main/docs/specification.md)
//! or [`Artifact.parts`](https://github.com/google/A2A/blob/main/docs/specification.md), resolve the
//! AgentCard skill definition for each part and invoke the bundle's Tier 3 WASM module keyed by
//! skill id. Parts without a matching skill program pass through unchanged.
//!
//! Roadmap: [`A2A_TODO.md`](../../../../A2A_TODO.md) §Phase 3 — push delivery & redaction.
//! Plan: [`A2A_PLAN.md`](../../../../A2A_PLAN.md) §Policy Engine (Tier 3 WASM redaction),
//! §Bundles (schema-driven redaction over `parts[*]` keyed by skill id),
//! §Decisions (policy substrate = single Wasmtime runtime hosting Tier 2 CEL and Tier 3 redaction).
//!
//! The shared Wasmtime host and Tier 2 CEL→WASM surface live in [`super::p2_wasmtime_substrate`].
//! This Phase 3 seam specializes Tier 3 for part-shaped payloads **without** pulling `wasmtime` into
//! `a2a-gateway` until bundle loading and signing land.

use std::fmt;

/// Skill-id selector for a Tier 3 redaction invocation over `Message.parts[*]` / `Artifact.parts[*]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillKeyedRedactionRequest {
    pub skill_id: String,
}

impl SkillKeyedRedactionRequest {
    pub fn new(skill_id: impl Into<String>) -> Self {
        Self {
            skill_id: skill_id.into(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.skill_id
    }
}

/// Outcome of applying a skill-keyed redaction program to a parts collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionVerdict {
    /// No WASM module configured or parts already conform — leave bytes unchanged.
    Passthrough,
    /// WASM redaction ran and removed or masked sensitive keys.
    Redacted { stripped_keys: usize },
}

impl RedactionVerdict {
    pub fn is_passthrough(&self) -> bool {
        matches!(self, Self::Passthrough)
    }

    pub fn stripped_keys(&self) -> Option<usize> {
        match self {
            Self::Passthrough => None,
            Self::Redacted { stripped_keys } => Some(*stripped_keys),
        }
    }
}

/// Errors surfaced before or during Tier 3 part redaction at the gateway gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactionGateError {
    /// Loaded WASM module skill id does not match the request or part hint.
    SkillMismatch { module_skill_id: String, requested: String },
    /// Tier 3 WASM substrate not wired (compile-only stub path).
    SubstrateNotWired,
}

impl fmt::Display for RedactionGateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SkillMismatch {
                module_skill_id,
                requested,
            } => write!(
                f,
                "tier 3 redaction skill mismatch: module={module_skill_id}, requested={requested}"
            ),
            Self::SubstrateNotWired => f.write_str("tier 3 wasm redaction substrate not wired"),
        }
    }
}

impl std::error::Error for RedactionGateError {}

/// Tier 3 WASM redaction over `Message.parts` / `Artifact.parts`, keyed by AgentCard skill id strings.
///
/// Implementations hold the resolved bundle module for one skill id. The optional `skill_id_hint`
/// supports parts that declare skill context out-of-band (for example catalog metadata on the
/// enclosing Message or Artifact).
pub trait PartRedactor: Send + Sync {
    fn skill_id(&self) -> &str;

    fn redact_skill_parts(
        &self,
        skill_id_hint: Option<&str>,
    ) -> Result<RedactionVerdict, RedactionGateError>;
}

/// Resolve the effective skill id and run Tier 3 redaction for a parts gate site.
pub fn redact_parts_for_skill<R: PartRedactor>(
    request: &SkillKeyedRedactionRequest,
    redactor: &R,
) -> Result<RedactionVerdict, RedactionGateError> {
    let requested = request.as_str();
    if requested != redactor.skill_id() {
        return Err(RedactionGateError::SkillMismatch {
            module_skill_id: redactor.skill_id().to_owned(),
            requested: requested.to_owned(),
        });
    }
    redactor.redact_skill_parts(Some(requested))
}

/// Compile-only passthrough redactor — exercises [`PartRedactor`] without Wasmtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubPassthroughPartRedactor {
    skill_id: String,
}

impl StubPassthroughPartRedactor {
    pub fn new(skill_id: impl Into<String>) -> Self {
        Self {
            skill_id: skill_id.into(),
        }
    }
}

impl PartRedactor for StubPassthroughPartRedactor {
    fn skill_id(&self) -> &str {
        &self.skill_id
    }

    fn redact_skill_parts(
        &self,
        skill_id_hint: Option<&str>,
    ) -> Result<RedactionVerdict, RedactionGateError> {
        let effective = skill_id_hint.unwrap_or(self.skill_id());
        if effective != self.skill_id() {
            return Err(RedactionGateError::SkillMismatch {
                module_skill_id: self.skill_id.clone(),
                requested: effective.to_owned(),
            });
        }
        Ok(RedactionVerdict::Passthrough)
    }
}

/// Compile-only redactor stub that reports a fixed strip count (no Wasmtime).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubRedactingPartRedactor {
    skill_id: String,
    stripped_keys: usize,
}

impl StubRedactingPartRedactor {
    pub fn new(skill_id: impl Into<String>, stripped_keys: usize) -> Self {
        Self {
            skill_id: skill_id.into(),
            stripped_keys,
        }
    }
}

impl PartRedactor for StubRedactingPartRedactor {
    fn skill_id(&self) -> &str {
        &self.skill_id
    }

    fn redact_skill_parts(
        &self,
        skill_id_hint: Option<&str>,
    ) -> Result<RedactionVerdict, RedactionGateError> {
        let effective = skill_id_hint.unwrap_or(self.skill_id());
        if effective != self.skill_id() {
            return Err(RedactionGateError::SkillMismatch {
                module_skill_id: self.skill_id.clone(),
                requested: effective.to_owned(),
            });
        }
        Ok(RedactionVerdict::Redacted {
            stripped_keys: self.stripped_keys,
        })
    }
}

/// Compile-only gate that always fails with [`RedactionGateError::SubstrateNotWired`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StubUnwiredPartRedactor;

impl PartRedactor for StubUnwiredPartRedactor {
    fn skill_id(&self) -> &str {
        ""
    }

    fn redact_skill_parts(
        &self,
        _skill_id_hint: Option<&str>,
    ) -> Result<RedactionVerdict, RedactionGateError> {
        Err(RedactionGateError::SubstrateNotWired)
    }
}
