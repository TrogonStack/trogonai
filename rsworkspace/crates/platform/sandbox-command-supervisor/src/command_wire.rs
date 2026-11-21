use crate::ExecutionId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandWire {
    Init {
        version: u32,
        execution_id: ExecutionId,
        argv: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
        timeout_ms: Option<u64>,
        release_timeout_ms: Option<u64>,
    },
    Release {
        execution_id: ExecutionId,
    },
    Signal {
        execution_id: ExecutionId,
        request_id: u64,
        signal: i32,
    },
    Status {
        execution_id: ExecutionId,
    },
    Acknowledge {
        execution_id: ExecutionId,
    },
}
