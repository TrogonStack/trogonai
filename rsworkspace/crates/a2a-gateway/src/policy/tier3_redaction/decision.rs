use a2a_redaction::SkillId;

use super::rewrite::RedactionRewrite;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Tier3RefusalReason {
    SkillPolicyDeniedPart,
    InvalidPayloadShape,
    UnauthorizedDataCategory,
}

impl Tier3RefusalReason {
    /// Convert a sentinel-emitted reason tag into the typed enum.
    ///
    /// Returns `None` for unknown tags — the caller decides whether to
    /// treat the unknown sentinel as `SkillPolicyDeniedPart`, route it
    /// as an engine error, or otherwise. Without this fallible boundary,
    /// any typo or new sentinel tag would silently map to
    /// `SkillPolicyDeniedPart` and mask engine/manifest mismatches as
    /// legitimate policy denials.
    pub fn from_sentinel_tag(tag: &str) -> Option<Self> {
        match tag {
            "SkillPolicyDeniedPart" => Some(Self::SkillPolicyDeniedPart),
            "InvalidPayloadShape" => Some(Self::InvalidPayloadShape),
            "UnauthorizedDataCategory" => Some(Self::UnauthorizedDataCategory),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SkillPolicyDeniedPart => "SkillPolicyDeniedPart",
            Self::InvalidPayloadShape => "InvalidPayloadShape",
            Self::UnauthorizedDataCategory => "UnauthorizedDataCategory",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Tier3EngineError {
    #[error("WasmTrap")]
    WasmTrap,
    #[error("WasmAbi")]
    WasmAbi,
    #[error("InvalidPayload")]
    InvalidPayload,
}

impl Tier3EngineError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WasmTrap => "WasmTrap",
            Self::WasmAbi => "WasmAbi",
            Self::InvalidPayload => "InvalidPayload",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tier3RedactionDecision {
    Allow { rewrites: Vec<RedactionRewrite> },
    Refuse { reason: Tier3RefusalReason, rule: SkillId },
    Error { rule: SkillId, kind: Tier3EngineError },
}

impl Tier3RedactionDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }
}
