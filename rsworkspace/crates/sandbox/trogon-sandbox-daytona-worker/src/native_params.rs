/// The create-time options of `native.daytona`, already validated.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct NativeParams {
    pub(crate) auto_stop_interval: Option<u32>,
    pub(crate) auto_archive_interval: Option<u32>,
    pub(crate) auto_delete_interval: Option<u32>,
    pub(crate) public: Option<bool>,
}
