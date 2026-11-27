/// KV bucket holding one manifest per captured snapshot.
pub(super) const BUCKET: &str = "sandbox-e2b-snapshot-executions";

/// Most commands one snapshot manifest may carry.
pub(crate) const MAX_EXECUTIONS: usize = 256;

pub(super) const MAX_MANIFEST_BYTES: usize = 60 * 1024;

pub(super) const MAX_TEMPLATE_ID_BYTES: usize = 128;

pub(super) const MAX_LOGICAL_NAME_BYTES: usize = 128;

pub(super) const MAX_PROCESS_IDENTITY_BYTES: usize = 128;
