use trogonai_proto::policy::subjects::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::rule_kind::RuleKind;
use crate::rule_ref::RuleRef;
use crate::stream_revision::StreamRevision;
use crate::subject::Subject;

impl TryFrom<&proto::PolicyRuleRef> for RuleRef {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::PolicyRuleRef) -> Result<Self, Self::Error> {
        let kind = value
            .kind
            .as_known()
            .ok_or(ProtoConversionError::UnknownEnumValue {
                enumeration: "PolicyRuleKind",
                value: value.kind.to_i32(),
            })
            .and_then(RuleKind::try_from)?;
        let subject = value.subject.as_option().ok_or(ProtoConversionError::MissingField {
            message: "PolicyRuleRef",
            field: "subject",
        })?;
        Ok(Self::new(
            kind,
            Subject::try_from(subject)?,
            StreamRevision::new(value.stream_revision),
        ))
    }
}
