use serde::{Deserialize, Serialize};

/// What happens to a destination the policy neither allows nor denies.
///
/// It serializes as the JSON strings `consult` and `refuse`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnlistedDestinations {
    /// Put it to the decider when there is one.
    #[default]
    Consult,
    /// Refuse it without asking anyone.
    Refuse,
}
