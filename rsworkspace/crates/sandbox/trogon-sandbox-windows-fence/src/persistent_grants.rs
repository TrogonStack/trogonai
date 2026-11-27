use crate::grant_state::GrantState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistentGrants {
    pub state: GrantState,
    pub roots: Vec<String>,
    pub reason: Option<String>,
}
