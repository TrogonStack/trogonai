use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableMessage {
    pub role: String, // "user" | "assistant"
    pub text: String,
}
