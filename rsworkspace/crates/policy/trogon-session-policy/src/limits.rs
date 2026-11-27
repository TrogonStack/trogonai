use serde::{Deserialize, Serialize};

use crate::PolicyError;

/// Bounded resource ceilings chosen by the trusted host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LimitsInput", into = "LimitsInput")]
pub struct Limits {
    command_seconds: u32,
    memory_mib: u32,
    processes: u32,
    payload_bytes: u32,
    model_calls: u32,
    tool_calls: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsInput {
    command_seconds: u32,
    memory_mib: u32,
    processes: u32,
    payload_bytes: u32,
    model_calls: u32,
    tool_calls: u32,
}

impl Limits {
    pub fn new(
        command_seconds: u32,
        memory_mib: u32,
        processes: u32,
        payload_bytes: u32,
        model_calls: u32,
        tool_calls: u32,
    ) -> Result<Self, PolicyError> {
        if !(1..=3600).contains(&command_seconds)
            || !(1..=131072).contains(&memory_mib)
            || !(1..=4096).contains(&processes)
            || !(1..=33554432).contains(&payload_bytes)
            || !(1..=1024).contains(&model_calls)
            || !(1..=1024).contains(&tool_calls)
        {
            return Err(PolicyError::Limits);
        }
        Ok(Self {
            command_seconds,
            memory_mib,
            processes,
            payload_bytes,
            model_calls,
            tool_calls,
        })
    }

    pub fn command_seconds(self) -> u32 {
        self.command_seconds
    }
    pub fn memory_mib(self) -> u32 {
        self.memory_mib
    }
    pub fn processes(self) -> u32 {
        self.processes
    }
    pub fn payload_bytes(self) -> u32 {
        self.payload_bytes
    }
    pub fn model_calls(self) -> u32 {
        self.model_calls
    }
    pub fn tool_calls(self) -> u32 {
        self.tool_calls
    }
}

impl TryFrom<LimitsInput> for Limits {
    type Error = PolicyError;
    fn try_from(value: LimitsInput) -> Result<Self, Self::Error> {
        Self::new(
            value.command_seconds,
            value.memory_mib,
            value.processes,
            value.payload_bytes,
            value.model_calls,
            value.tool_calls,
        )
    }
}

impl From<Limits> for LimitsInput {
    fn from(value: Limits) -> Self {
        Self {
            command_seconds: value.command_seconds,
            memory_mib: value.memory_mib,
            processes: value.processes,
            payload_bytes: value.payload_bytes,
            model_calls: value.model_calls,
            tool_calls: value.tool_calls,
        }
    }
}
