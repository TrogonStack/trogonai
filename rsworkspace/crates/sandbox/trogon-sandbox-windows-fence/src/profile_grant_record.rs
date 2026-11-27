use crate::grant_state::GrantState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGrantRecord {
    pub state: GrantState,
    pub reason: Option<String>,
}
