use serde::{Deserialize, Serialize};

/// The body of a minted session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub token: String,
    pub expires_at: u64,
}
