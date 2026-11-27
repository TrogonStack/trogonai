use serde::{Deserialize, Serialize};

use super::ActorUidError;
use super::constants::MAX_ACTOR_UID_LEN;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ActorUid(String);

impl ActorUid {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ActorUid {
    type Error = ActorUidError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > MAX_ACTOR_UID_LEN
            || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(ActorUidError);
        }
        Ok(Self(value))
    }
}

impl From<ActorUid> for String {
    fn from(value: ActorUid) -> Self {
        value.0
    }
}
