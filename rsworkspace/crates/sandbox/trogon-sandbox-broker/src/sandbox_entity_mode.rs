use std::str::FromStr;

use crate::sandbox_entity_mode_error::SandboxEntityModeError;

/// How the broker uses the entity store. `Off` never touches it. `Shadow`
/// mirrors named persistent leases into it without affecting any response.
/// `On` makes the entity the source of truth for named persistent sandboxes:
/// an entity failure fails the request with a typed error, names resolve
/// through the entity, and the entity get, list and delete verbs are served.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum SandboxEntityMode {
    #[default]
    Off,
    Shadow,
    On,
}

impl FromStr for SandboxEntityMode {
    type Err = SandboxEntityModeError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "shadow" => Ok(Self::Shadow),
            "on" => Ok(Self::On),
            other => Err(SandboxEntityModeError {
                value: other.to_string(),
            }),
        }
    }
}
