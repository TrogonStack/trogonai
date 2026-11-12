use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
pub(crate) enum PromptHookEvent {
    UserPromptSubmit,
}
