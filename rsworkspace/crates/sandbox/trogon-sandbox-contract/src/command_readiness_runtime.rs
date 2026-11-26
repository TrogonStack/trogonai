use std::num::NonZeroU32;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::Digest;
use crate::command_readiness_runtime_wire::CommandReadinessRuntimeWire;
use crate::constants::DIGEST_PREFIX;

/// The sealed supervisor a sandbox runs, as a command readiness probe found it.
///
/// `sha256` serializes as the bare 64-character hex hash, without the `sha256:` prefix a [`Digest`] carries.
/// Deserialization rejects unknown fields.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct CommandReadinessRuntime {
    pub sha256: Digest,
    pub protocol: NonZeroU32,
}

impl CommandReadinessRuntime {
    /// The command readiness protocol the sealed supervisor speaks. A provider exposes strong command features only
    /// when a probe reports this protocol.
    pub const PROTOCOL: NonZeroU32 = NonZeroU32::MIN;
}

impl Serialize for CommandReadinessRuntime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        CommandReadinessRuntimeWire {
            sha256: self.sha256.hex().to_owned(),
            protocol: self.protocol,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CommandReadinessRuntime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CommandReadinessRuntimeWire::deserialize(deserializer)?;
        let sha256 = Digest::new(format!("{DIGEST_PREFIX}{}", wire.sha256)).map_err(de::Error::custom)?;
        Ok(Self {
            sha256,
            protocol: wire.protocol,
        })
    }
}
