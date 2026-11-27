//! Routes whose behavior goes beyond decode, authorize and answer.

pub(crate) mod cancel;
pub(crate) mod capability_list;
pub(crate) mod command_reply;
pub(crate) mod file_op;
pub(crate) mod file_transfer;
pub(crate) mod lease_grant;
pub(crate) mod operation_capabilities_reply;
pub(crate) mod snapshot_events;
pub(crate) mod worker;
