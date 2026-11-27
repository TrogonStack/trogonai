use serde::Deserialize;
use trogon_sandbox_contract::SandboxId;

/// One Modal Sandbox as the bridge reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModalSandbox {
    pub id: SandboxId,
    pub state: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

impl ModalSandbox {
    /// Modal's lifecycle is `Created`, `Scheduled`, `Started`, `Ready`, `Finished`, and `exec` is valid from
    /// `Started` onward. The bridge collapses that to `running` and `finished`, but the richer spellings are
    /// accepted so a transport reporting raw states needs no change here.
    pub fn is_running(&self) -> bool {
        matches!(
            self.state.to_ascii_lowercase().as_str(),
            "running" | "ready" | "started" | "active"
        )
    }
}
