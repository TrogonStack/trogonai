use crate::sublayer_guid::SublayerGuid;
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallOptions {
    pub sublayer_guid: Option<SublayerGuid>,
    pub keep_user: bool,
    pub timeout_ms: Option<u64>,
}
