use serde::Deserialize;

/// What E2B returns when a sandbox is created or resumed.
#[derive(Clone, Deserialize)]
pub struct SandboxInfo {
    #[serde(rename = "sandboxID")]
    pub sandbox_id: String,
    #[serde(rename = "templateID", default)]
    pub template_id: String,
    #[serde(rename = "envdAccessToken", default)]
    pub envd_access_token: Option<String>,
    #[serde(rename = "trafficAccessToken", default)]
    pub traffic_access_token: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
}

impl std::fmt::Debug for SandboxInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxInfo")
            .field("sandbox_id", &self.sandbox_id)
            .field("template_id", &self.template_id)
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}
