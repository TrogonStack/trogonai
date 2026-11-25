use trogonai_proto::policy::egress_permissions::v1alpha1 as proto;
use trogonai_proto::policy::egress_permissions::v1alpha1::__buffa::oneof::egress_destination::Destination as Arm;

use super::ProtoConversionError;
use crate::egress_destination::EgressDestination;
use crate::egress_host_name::EgressHostName;
use crate::port::Port;

impl TryFrom<&proto::EgressHost> for EgressDestination {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::EgressHost) -> Result<Self, Self::Error> {
        Ok(Self::new(
            EgressHostName::new(value.host.as_str())?,
            Port::try_from(value.port)?,
        ))
    }
}

impl TryFrom<&proto::EgressDestination> for EgressDestination {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::EgressDestination) -> Result<Self, Self::Error> {
        match value.destination.as_ref() {
            Some(Arm::Host(host)) => Self::try_from(host.as_ref()),
            None => Err(ProtoConversionError::MissingField {
                message: "EgressDestination",
                field: "destination",
            }),
        }
    }
}
