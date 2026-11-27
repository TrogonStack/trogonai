use trogon_sandbox_contract::{Base64Data, FileParentCreation, FileWriteTruncation};

/// The fields of a write operation.
pub(super) struct WriteSpec<'a> {
    pub(super) path: &'a str,
    pub(super) data: &'a Base64Data,
    pub(super) offset: u64,
    pub(super) truncate: FileWriteTruncation,
    pub(super) mode: Option<u32>,
    pub(super) create_parents: FileParentCreation,
}
