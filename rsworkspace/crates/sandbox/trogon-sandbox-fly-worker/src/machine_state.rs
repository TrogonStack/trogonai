use serde::{Deserialize, Serialize};

/// The lifecycle state Fly reports for a Machine.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum MachineState {
    Started,
    Suspended,
    Stopped,
    Destroyed,
    Failed,
    Other(String),
}

impl From<String> for MachineState {
    fn from(value: String) -> Self {
        match value.as_str() {
            "started" => Self::Started,
            "suspended" => Self::Suspended,
            "stopped" => Self::Stopped,
            "destroyed" => Self::Destroyed,
            "failed" => Self::Failed,
            _ => Self::Other(value),
        }
    }
}

impl From<MachineState> for String {
    fn from(value: MachineState) -> Self {
        match value {
            MachineState::Started => "started".into(),
            MachineState::Suspended => "suspended".into(),
            MachineState::Stopped => "stopped".into(),
            MachineState::Destroyed => "destroyed".into(),
            MachineState::Failed => "failed".into(),
            MachineState::Other(other) => other,
        }
    }
}
