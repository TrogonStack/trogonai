use trogonai_proto::policy::holds::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::hold_name::HoldName;

impl TryFrom<&proto::HoldName> for HoldName {
    type Error = ProtoConversionError;

    /// Applies the domain rules only. The schema's stricter lowercase pattern
    /// is documentation (ADR#0064), and a name the placing side accepted is
    /// not refused here for a rule this crate does not own.
    fn try_from(value: &proto::HoldName) -> Result<Self, Self::Error> {
        Self::new(value.value.as_str()).map_err(ProtoConversionError::InvalidHoldName)
    }
}
