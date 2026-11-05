use trogonai_proto::policy::subjects::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::rule_kind::RuleKind;

impl TryFrom<proto::PolicyRuleKind> for RuleKind {
    type Error = ProtoConversionError;

    /// Maps the kinds this evaluator reads. Budgets, rate limits, and
    /// connection permissions are real kinds in the schema that no
    /// enforcement point evaluates here, so they are refused rather than
    /// folded into a kind they are not.
    fn try_from(value: proto::PolicyRuleKind) -> Result<Self, Self::Error> {
        match value {
            proto::PolicyRuleKind::POLICY_RULE_KIND_HOLD => Ok(Self::Hold),
            proto::PolicyRuleKind::POLICY_RULE_KIND_MODEL_CEILING => Ok(Self::ModelCeiling),
            proto::PolicyRuleKind::POLICY_RULE_KIND_DELEGATION_PERMISSION => Ok(Self::DelegationPermission),
            proto::PolicyRuleKind::POLICY_RULE_KIND_EGRESS_PERMISSION => Ok(Self::EgressPermission),
            proto::PolicyRuleKind::POLICY_RULE_KIND_UNSPECIFIED => Err(ProtoConversionError::UnknownEnumValue {
                enumeration: "PolicyRuleKind",
                value: value as i32,
            }),
            proto::PolicyRuleKind::POLICY_RULE_KIND_AGENT_BUDGET
            | proto::PolicyRuleKind::POLICY_RULE_KIND_AGENT_RATE_LIMIT
            | proto::PolicyRuleKind::POLICY_RULE_KIND_CONNECTION_PERMISSION => {
                Err(ProtoConversionError::UnsupportedRuleKind { value: value as i32 })
            }
        }
    }
}
