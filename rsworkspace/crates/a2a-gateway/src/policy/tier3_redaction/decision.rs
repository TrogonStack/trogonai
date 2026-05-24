use a2a_redaction::SkillId;

use super::rewrite::RedactionRewrite;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Tier3RefusalReason {
    SkillPolicyDeniedPart,
    InvalidPayloadShape,
    UnauthorizedDataCategory,
}

impl Tier3RefusalReason {
    pub fn from_sentinel_tag(tag: &str) -> Self {
        match tag {
            "InvalidPayloadShape" => Self::InvalidPayloadShape,
            "UnauthorizedDataCategory" => Self::UnauthorizedDataCategory,
            _ => Self::SkillPolicyDeniedPart,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Tier3EngineError {
    WasmTrap,
    WasmAbi,
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
    Allow {
        rewrites: Vec<RedactionRewrite>,
    },
    Refuse {
        reason: Tier3RefusalReason,
        rule: SkillId,
    },
    Error {
        rule: SkillId,
        kind: Tier3EngineError,
    },
}

impl Tier3RedactionDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }
}
