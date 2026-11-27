/// The create-time options of `native.daytona`, already validated.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(crate) struct NativeParams {
    pub(crate) auto_stop_interval: Option<u32>,
    pub(crate) auto_archive_interval: Option<u32>,
    pub(crate) auto_delete_interval: Option<u32>,
    pub(crate) public: Option<bool>,
    pub(crate) class: Option<String>,
    pub(crate) gpu: Option<u32>,
}
