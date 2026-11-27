use trogon_sandbox_contract::FileOp;

/// A search request: the guest path to collect from, and the same search rooted at a staging directory.
pub(super) struct SearchSpec<'a> {
    pub(super) path: &'a str,
    pub(super) glob: Option<&'a str>,
    pub(super) searches_content: bool,
    pub(super) max_file_bytes: Option<u64>,
    pub(super) staged: FileOp,
}
