use serde::Deserialize;
use serde_json::Value;

use crate::{MachineId, MachineState};

/// A Fly Machine as the Machines API reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct Machine {
    pub id: MachineId,
    pub state: MachineState,
    pub config: Value,
}

impl Machine {
    /// A string entry of the Machine's `config.metadata`.
    pub fn metadata(&self, key: &str) -> Option<&str> {
        self.config["metadata"][key].as_str()
    }
}
