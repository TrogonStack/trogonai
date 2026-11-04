use serde::Serialize;

use crate::prompt_hook_context::PromptHookContext;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptHookOutput<'a> {
    pub(crate) hook_specific_output: PromptHookContext<'a>,
}
