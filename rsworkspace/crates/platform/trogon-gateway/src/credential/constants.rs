use uuid::Uuid;

pub(crate) const CREDENTIAL_STREAM: &str = "GATEWAY_CREDENTIAL_EVENTS";
pub(crate) const CREDENTIAL_EVENT_SUBJECT_PREFIX: &str = "gateway.credentials.events.v1";
pub(super) const CREDENTIAL_NAMESPACE: Uuid = Uuid::from_u128(0x73c6_35ee_2d9e_4a07_a77a_9b0b_6a87_3171);
pub(crate) const CREDENTIAL_SNAPSHOT_BUCKET: &str = "GATEWAY_CREDENTIAL_SNAPSHOTS";
