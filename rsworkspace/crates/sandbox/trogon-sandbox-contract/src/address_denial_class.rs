use serde::{Deserialize, Serialize};

/// Which class of the SSRF floor refused a destination address.
///
/// The classes are ordered by reporting priority: when several resolved addresses are refused, the smallest class
/// is the one reported.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressDenialClass {
    DenyListed,
    Loopback,
    Unspecified,
    LinkLocal,
    CloudMetadata,
    Multicast,
    Broadcast,
    OwnInterface,
    PrivateUse,
    Listed,
    NonLoopback,
}
