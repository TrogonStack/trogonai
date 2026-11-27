use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct WfpVerifyWire {
    pub(crate) egress_probe: Option<String>,
    #[serde(default)]
    pub(crate) target: String,
    pub(crate) anonymous_probe: Option<String>,
    pub(crate) anonymous_target: Option<String>,
    pub(crate) anonymous_loopback_probe: Option<String>,
    pub(crate) deny_only_probe: Option<String>,
    pub(crate) low_integrity_probe: Option<String>,
    pub(crate) low_integrity_loopback_probe: Option<String>,
    pub(crate) untrusted_integrity_probe: Option<String>,
    pub(crate) untrusted_integrity_loopback_probe: Option<String>,
    pub(crate) egress_target: Option<String>,
    pub(crate) allow_loopback: Option<bool>,
    pub(crate) open_loopback_probe: Option<String>,
    pub(crate) residuals: Option<Vec<String>>,
    pub(crate) allow_loopback_mismatch: Option<bool>,
    pub(crate) hint: Option<String>,
}
