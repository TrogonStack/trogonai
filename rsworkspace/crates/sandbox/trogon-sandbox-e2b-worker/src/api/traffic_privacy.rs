use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Whether a sandbox's public ports are open to anyone, as E2B reports it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TrafficPrivacy {
    Public,
    Private,
    #[default]
    Unknown,
}

impl<'de> Deserialize<'de> for TrafficPrivacy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Value::deserialize(deserializer)? {
            Value::Null => Ok(Self::Unknown),
            Value::Object(network) => match network.get("allowPublicTraffic") {
                None | Some(Value::Null) => Ok(Self::Unknown),
                Some(Value::Bool(true)) => Ok(Self::Public),
                Some(Value::Bool(false)) => Ok(Self::Private),
                _ => Err(serde::de::Error::custom("native traffic privacy flag is invalid")),
            },
            _ => Err(serde::de::Error::custom("native traffic privacy metadata is invalid")),
        }
    }
}
