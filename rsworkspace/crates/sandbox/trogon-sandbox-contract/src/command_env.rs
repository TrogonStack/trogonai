use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::CommandEnvError;

/// The environment variables a command runs with.
///
/// A worker sets [`CommandEnv::RESERVED_KEY`] itself, so an environment that names it is rejected on construction and
/// on deserialization. It serializes as a JSON object of strings.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "BTreeMap<String, String>", into = "BTreeMap<String, String>")]
pub struct CommandEnv(BTreeMap<String, String>);

impl CommandEnv {
    /// The variable a worker sets to the execution id, which a request may not set.
    pub const RESERVED_KEY: &'static str = crate::constants::EXECUTION_ID_ENV;

    pub fn new(vars: BTreeMap<String, String>) -> Result<Self, CommandEnvError> {
        if vars.contains_key(Self::RESERVED_KEY) {
            return Err(CommandEnvError::ReservedKey);
        }
        Ok(Self(vars))
    }

    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    pub fn iter(&self) -> std::collections::btree_map::Iter<'_, String, String> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<BTreeMap<String, String>> for CommandEnv {
    type Error = CommandEnvError;

    fn try_from(vars: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        Self::new(vars)
    }
}

impl From<CommandEnv> for BTreeMap<String, String> {
    fn from(env: CommandEnv) -> Self {
        env.0
    }
}
