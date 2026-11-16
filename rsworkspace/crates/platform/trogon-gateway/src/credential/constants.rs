use uuid::Uuid;

pub(crate) use trogon_credential_aggregate::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM};

pub(super) const CREDENTIAL_NAMESPACE: Uuid = Uuid::from_u128(0x73c6_35ee_2d9e_4a07_a77a_9b0b_6a87_3171);
pub(crate) const CREDENTIAL_SNAPSHOT_BUCKET: &str = "CREDENTIAL_SNAPSHOTS";
