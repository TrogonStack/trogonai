use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLimitMechanism {
    #[default]
    None,
    Rlimit,
    CgroupV2,
    JobObject,
    Runtime,
}
