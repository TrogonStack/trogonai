use serde::Deserialize;

use super::{NativeAccessToken, TrafficPrivacy};

/// What `GET /sandboxes/{id}` says about a sandbox being adopted after a restart.
#[derive(Debug, Deserialize)]
pub(crate) struct NativeSandboxAccess {
    #[serde(rename = "sandboxID")]
    pub(crate) sandbox_id: String,
    pub(crate) state: String,
    #[serde(rename = "endAt")]
    pub(crate) end_at: String,
    #[serde(rename = "envdAccessToken", default)]
    pub(crate) token: Option<NativeAccessToken>,
    #[serde(default)]
    pub(crate) domain: Option<String>,
    #[serde(rename = "network", default)]
    pub(crate) traffic_privacy: TrafficPrivacy,
}
